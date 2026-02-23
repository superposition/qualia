import WebSocket from 'ws';
import { signMessage } from '@qualia/passport';
import {
  A2AClientConfig,
  RequestParams,
  JsonRpcRequest,
  JsonRpcResponse,
} from './types';
import { discover as discoverAgents, getAgentMetadata } from './discovery';

/**
 * A2AClient - Client for sending JSON-RPC requests to other agents
 *
 * Handles agent discovery, establishes WebSocket connections,
 * signs requests with Ed25519 DID-based authentication, and manages
 * request/response flow.
 */
export class A2AClient {
  private config: A2AClientConfig;
  private connections: Map<string, WebSocket> = new Map();
  private pendingRequests: Map<
    string | number,
    {
      resolve: (value: any) => void;
      reject: (error: Error) => void;
      timeout: NodeJS.Timeout;
    }
  > = new Map();
  private requestIdCounter = 0;

  constructor(config: A2AClientConfig) {
    this.config = config;
  }

  /**
   * Send a JSON-RPC request to another agent
   * @param params - Request parameters including target agent, method, and params
   * @returns Promise that resolves with the response result
   */
  async request(params: RequestParams): Promise<any> {
    const { to, method, params: methodParams, timeout = 30000 } = params;

    // Resolve agent address (DID, NANDA name, or direct URL)
    const agentUrl = await this.resolveAgent(to);

    // Get or create WebSocket connection
    const ws = await this.getConnection(agentUrl);

    // Generate request ID
    const requestId = this.generateRequestId();

    // Sign the request payload with Ed25519
    const payloadToSign = { method, params: methodParams };
    const signature = await signMessage(payloadToSign, this.config.privateKey);

    // Create JSON-RPC request
    const request: JsonRpcRequest = {
      jsonrpc: '2.0',
      method,
      params: methodParams,
      auth: {
        from: this.config.did,
        signature,
      },
      id: requestId,
    };

    // Send request and wait for response
    return new Promise((resolve, reject) => {
      const timeoutHandle = setTimeout(() => {
        this.pendingRequests.delete(requestId);
        reject(
          new Error(
            `Request timeout after ${timeout}ms for method: ${method}`
          )
        );
      }, timeout);

      this.pendingRequests.set(requestId, {
        resolve,
        reject,
        timeout: timeoutHandle,
      });

      try {
        ws.send(JSON.stringify(request));
      } catch (error) {
        clearTimeout(timeoutHandle);
        this.pendingRequests.delete(requestId);
        reject(error);
      }
    });
  }

  /**
   * Discover agents by capability
   * @param capability - The capability to search for
   * @returns Array of agent DIDs
   */
  async discover(capability: string): Promise<string[]> {
    return discoverAgents(capability);
  }

  /**
   * Close all connections and cleanup
   */
  async close(): Promise<void> {
    this.pendingRequests.forEach(({ reject, timeout }) => {
      clearTimeout(timeout);
      reject(new Error('Client closed'));
    });
    this.pendingRequests.clear();

    const closePromises = Array.from(this.connections.values()).map(
      (ws) =>
        new Promise<void>((resolve) => {
          if (ws.readyState === WebSocket.OPEN || ws.readyState === WebSocket.CONNECTING) {
            const timeout = setTimeout(() => {
              ws.removeAllListeners('close');
              resolve();
            }, 1000);

            ws.once('close', () => {
              clearTimeout(timeout);
              resolve();
            });
            ws.close();
          } else {
            resolve();
          }
        })
    );

    await Promise.all(closePromises);
    this.connections.clear();
  }

  /**
   * Resolve agent identifier to WebSocket URL
   */
  private async resolveAgent(identifier: string): Promise<string> {
    // Direct WebSocket URL
    if (identifier.startsWith('ws://') || identifier.startsWith('wss://')) {
      return identifier;
    }

    // Resolve DID via discovery registry
    if (identifier.startsWith('did:')) {
      const metadata = await getAgentMetadata(identifier);
      if (metadata?.endpoints.a2a) {
        return metadata.endpoints.a2a;
      }
      throw new Error(`Agent not found or has no A2A endpoint: ${identifier}`);
    }

    // Treat as capability and discover
    const agents = await discoverAgents(identifier);
    if (agents.length === 0) {
      throw new Error(`No agents found with capability: ${identifier}`);
    }

    // Resolve first matching agent
    const metadata = await getAgentMetadata(agents[0]!);
    if (metadata?.endpoints.a2a) {
      return metadata.endpoints.a2a;
    }

    throw new Error(`Agent ${agents[0]} has no A2A endpoint`);
  }

  private async getConnection(url: string): Promise<WebSocket> {
    const existing = this.connections.get(url);
    if (existing && existing.readyState === WebSocket.OPEN) {
      return existing;
    }

    return new Promise((resolve, reject) => {
      const ws = new WebSocket(url);

      ws.on('open', () => {
        this.connections.set(url, ws);
        this.setupMessageHandler(ws);
        resolve(ws);
      });

      ws.on('error', (error) => {
        reject(error);
      });

      ws.on('close', () => {
        this.connections.delete(url);
      });
    });
  }

  private setupMessageHandler(ws: WebSocket): void {
    ws.on('message', (data: Buffer) => {
      try {
        const response = JSON.parse(data.toString()) as JsonRpcResponse;
        const pending = this.pendingRequests.get(response.id);

        if (pending) {
          clearTimeout(pending.timeout);
          this.pendingRequests.delete(response.id);

          if (response.error) {
            pending.reject(
              new Error(
                `JSON-RPC Error ${response.error.code}: ${response.error.message}`
              )
            );
          } else {
            pending.resolve(response.result);
          }
        }
      } catch (error) {
        // Ignore unparseable responses
      }
    });
  }

  private generateRequestId(): string {
    return `req-${++this.requestIdCounter}-${Date.now()}`;
  }
}
