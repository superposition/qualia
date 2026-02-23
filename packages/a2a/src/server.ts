import WebSocket, { WebSocketServer } from 'ws';
import { signMessage, verifyMessage } from '@qualia/passport';
import type { DID } from '@qualia/types';
import {
  A2AServerConfig,
  JsonRpcRequest,
  JsonRpcResponse,
  JsonRpcErrorCode,
  MethodHandler,
} from './types';

/**
 * A2AServer - WebSocket server for agent-to-agent JSON-RPC communication
 *
 * Handles incoming JSON-RPC requests from other agents, verifies their
 * DID-based authentication, routes to registered method handlers, and
 * sends back responses.
 */
export class A2AServer {
  private config: A2AServerConfig;
  private wss: WebSocketServer | null = null;
  private methodHandlers: Map<string, MethodHandler> = new Map();
  private clients: Map<WebSocket, string> = new Map();

  constructor(config: A2AServerConfig) {
    this.config = {
      host: '0.0.0.0',
      requireAuth: true,
      ...config,
    };
  }

  /**
   * Register a method handler for incoming RPC calls
   * @param method - The method name to handle
   * @param handler - Async function that processes the request
   */
  on(method: string, handler: MethodHandler): void {
    this.methodHandlers.set(method, handler);
  }

  /**
   * Start the WebSocket server
   */
  async start(): Promise<void> {
    return new Promise((resolve, reject) => {
      try {
        this.wss = new WebSocketServer({
          host: this.config.host,
          port: this.config.port,
        });

        this.wss.on('listening', () => {
          resolve();
        });

        this.wss.on('connection', (ws: WebSocket) => {
          this.handleConnection(ws);
        });

        this.wss.on('error', (error) => {
          reject(error);
        });
      } catch (error) {
        reject(error);
      }
    });
  }

  /**
   * Stop the WebSocket server
   */
  async stop(): Promise<void> {
    return new Promise((resolve) => {
      if (this.wss) {
        this.clients.forEach((_did, ws) => {
          ws.close();
        });
        this.clients.clear();

        const timeout = setTimeout(() => {
          this.wss = null;
          resolve();
        }, 500);

        this.wss.close(() => {
          clearTimeout(timeout);
          this.wss = null;
          resolve();
        });
      } else {
        resolve();
      }
    });
  }

  private handleConnection(ws: WebSocket): void {
    ws.on('message', async (data: Buffer) => {
      try {
        const request = JSON.parse(data.toString()) as JsonRpcRequest;
        const response = await this.handleRequest(request, ws);
        ws.send(JSON.stringify(response));
      } catch (error) {
        const errorResponse: JsonRpcResponse = {
          jsonrpc: '2.0',
          error: {
            code: JsonRpcErrorCode.PARSE_ERROR,
            message: 'Invalid JSON',
            data: error instanceof Error ? error.message : String(error),
          },
          id: -1,
        };
        ws.send(JSON.stringify(errorResponse));
      }
    });

    ws.on('close', () => {
      this.clients.delete(ws);
    });

    ws.on('error', () => {
      this.clients.delete(ws);
    });
  }

  private async handleRequest(
    request: JsonRpcRequest,
    ws: WebSocket
  ): Promise<JsonRpcResponse> {
    // Validate JSON-RPC format
    if (request.jsonrpc !== '2.0' || !request.method || !request.id) {
      return {
        jsonrpc: '2.0',
        error: {
          code: JsonRpcErrorCode.INVALID_REQUEST,
          message: 'Invalid JSON-RPC request',
        },
        id: request.id ?? -1,
      };
    }

    let senderDid = 'anonymous';

    // Verify authentication if required
    if (this.config.requireAuth) {
      if (!request.auth || !request.auth.from || !request.auth.signature) {
        return {
          jsonrpc: '2.0',
          error: {
            code: JsonRpcErrorCode.AUTHENTICATION_FAILED,
            message: 'Missing authentication',
          },
          id: request.id,
        };
      }

      // Verify Ed25519 DID signature
      const payloadToVerify = {
        method: request.method,
        params: request.params,
      };

      const isValid = await verifyMessage(
        request.auth.from as DID,
        request.auth.signature,
        payloadToVerify
      );

      if (!isValid) {
        return {
          jsonrpc: '2.0',
          error: {
            code: JsonRpcErrorCode.AUTHENTICATION_FAILED,
            message: 'Invalid signature',
          },
          id: request.id,
        };
      }

      senderDid = request.auth.from;
    } else if (request.auth?.from) {
      senderDid = request.auth.from;
    }

    // Store client DID
    this.clients.set(ws, senderDid);

    // Find method handler
    const handler = this.methodHandlers.get(request.method);
    if (!handler) {
      return {
        jsonrpc: '2.0',
        error: {
          code: JsonRpcErrorCode.METHOD_NOT_FOUND,
          message: `Method not found: ${request.method}`,
        },
        id: request.id,
      };
    }

    // Execute handler
    try {
      const result = await handler(request.params, senderDid);
      return {
        jsonrpc: '2.0',
        result,
        id: request.id,
      };
    } catch (error) {
      return {
        jsonrpc: '2.0',
        error: {
          code: JsonRpcErrorCode.INTERNAL_ERROR,
          message: error instanceof Error ? error.message : 'Internal error',
        },
        id: request.id,
      };
    }
  }

  /**
   * Get list of connected client DIDs
   */
  getConnectedClients(): string[] {
    return Array.from(this.clients.values());
  }
}
