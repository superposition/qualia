import WebSocket from 'ws';
import {
  A2AClientConfig,
  RequestParams,
  JsonRpcRequest,
  JsonRpcResponse,
} from './types';
import { signRequest } from './__mocks__/passport';
import { discover as discoverAgents } from './discovery';

/** Client connection events */
export type ClientEvent = 'connected' | 'disconnected' | 'reconnecting';

/** Client event listener */
export type ClientEventListener = (url: string) => void;

/** Auto-reconnect configuration */
export interface ReconnectConfig {
  /** Enable auto-reconnect (default: false) */
  enabled?: boolean;
  /** Maximum number of retry attempts (default: 5) */
  maxRetries?: number;
  /** Initial retry delay in ms (default: 1000) */
  initialDelayMs?: number;
  /** Maximum retry delay in ms (default: 30000) */
  maxDelayMs?: number;
}

/**
 * A2AClient - Client for sending JSON-RPC requests to other agents
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
  private eventListeners: Map<ClientEvent, Set<ClientEventListener>> = new Map();
  private reconnectConfig: ReconnectConfig;
  private reconnectAttempts: Map<string, number> = new Map();
  private closed = false;

  constructor(config: A2AClientConfig, reconnect?: ReconnectConfig) {
    this.config = config;
    this.reconnectConfig = {
      enabled: reconnect?.enabled ?? false,
      maxRetries: reconnect?.maxRetries ?? 5,
      initialDelayMs: reconnect?.initialDelayMs ?? 1000,
      maxDelayMs: reconnect?.maxDelayMs ?? 30000,
    };
  }

  /**
   * Register a connection event listener
   */
  onEvent(event: ClientEvent, listener: ClientEventListener): () => void {
    if (!this.eventListeners.has(event)) {
      this.eventListeners.set(event, new Set());
    }
    this.eventListeners.get(event)!.add(listener);
    return () => {
      this.eventListeners.get(event)?.delete(listener);
    };
  }

  /**
   * Send a JSON-RPC request to another agent
   */
  async request(params: RequestParams): Promise<any> {
    const { to, method, params: methodParams, timeout = 30000 } = params;

    const agentUrl = await this.resolveAgent(to);
    const ws = await this.getConnection(agentUrl);
    const requestId = this.generateRequestId();

    const request: JsonRpcRequest = {
      jsonrpc: '2.0',
      method,
      params: methodParams,
      auth: {
        from: this.config.did,
        signature: await signRequest(this.config.privateKey, {
          method,
          params: methodParams,
        }),
      },
      id: requestId,
    };

    return new Promise((resolve, reject) => {
      const timeoutHandle = setTimeout(() => {
        this.pendingRequests.delete(requestId);
        reject(
          new Error(
            `Request timeout after ${timeout}ms for method: ${method}`,
          ),
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
   */
  async discover(capability: string): Promise<string[]> {
    return discoverAgents(capability);
  }

  /**
   * Close all connections and cleanup
   */
  async close(): Promise<void> {
    this.closed = true;

    this.pendingRequests.forEach(({ reject, timeout }) => {
      clearTimeout(timeout);
      reject(new Error('Client closed'));
    });
    this.pendingRequests.clear();

    const closePromises = Array.from(this.connections.values()).map(
      (ws) =>
        new Promise<void>((resolve) => {
          if (
            ws.readyState === WebSocket.OPEN ||
            ws.readyState === WebSocket.CONNECTING
          ) {
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
        }),
    );

    await Promise.all(closePromises);
    this.connections.clear();
    this.reconnectAttempts.clear();
  }

  private async resolveAgent(identifier: string): Promise<string> {
    if (identifier.startsWith('ws://') || identifier.startsWith('wss://')) {
      return identifier;
    }

    if (identifier.startsWith('did:')) {
      const agents = await this.discover('*');
      if (agents.includes(identifier)) {
        return `ws://localhost:8080`;
      }
      throw new Error(`Agent not found: ${identifier}`);
    }

    const agents = await this.discover(identifier);
    if (agents.length === 0) {
      throw new Error(`No agents found with capability: ${identifier}`);
    }

    return `ws://localhost:8080`;
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
        this.reconnectAttempts.delete(url);
        this.setupMessageHandler(ws);
        this.emitEvent('connected', url);
        resolve(ws);
      });

      ws.on('error', (error) => {
        reject(error);
      });

      ws.on('close', () => {
        this.connections.delete(url);
        this.emitEvent('disconnected', url);

        if (this.reconnectConfig.enabled && !this.closed) {
          this.scheduleReconnect(url);
        }
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
                `JSON-RPC Error ${response.error.code}: ${response.error.message}`,
              ),
            );
          } else {
            pending.resolve(response.result);
          }
        }
      } catch (_error) {
        // Failed to parse response
      }
    });
  }

  private scheduleReconnect(url: string): void {
    const attempts = this.reconnectAttempts.get(url) ?? 0;
    if (attempts >= (this.reconnectConfig.maxRetries ?? 5)) {
      return;
    }

    const delay = Math.min(
      (this.reconnectConfig.initialDelayMs ?? 1000) * Math.pow(2, attempts),
      this.reconnectConfig.maxDelayMs ?? 30000,
    );

    this.reconnectAttempts.set(url, attempts + 1);
    this.emitEvent('reconnecting', url);

    setTimeout(async () => {
      if (this.closed) return;
      try {
        await this.getConnection(url);
      } catch {
        // Will retry via close handler
      }
    }, delay);
  }

  private emitEvent(event: ClientEvent, url: string): void {
    const listeners = this.eventListeners.get(event);
    if (listeners) {
      for (const listener of listeners) {
        listener(url);
      }
    }
  }

  private generateRequestId(): string {
    return `req-${++this.requestIdCounter}-${Date.now()}`;
  }
}
