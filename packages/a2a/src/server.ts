import WebSocket, { WebSocketServer } from 'ws';
import {
  A2AServerConfig,
  JsonRpcRequest,
  JsonRpcResponse,
  JsonRpcErrorCode,
  MethodHandler,
} from './types';
import { verifyPassport } from './__mocks__/passport';
import {
  Middleware,
  MiddlewareContext,
  composeMiddleware,
} from './middleware';

/** Connection events emitted by the server */
export type ServerEvent = 'client:connected' | 'client:disconnected';

/** Server event listener */
export type ServerEventListener = (did: string) => void;

/** Heartbeat configuration */
export interface HeartbeatConfig {
  /** Interval in ms between pings (default: 30000) */
  intervalMs?: number;
  /** Time in ms to wait for pong before considering dead (default: 10000) */
  timeoutMs?: number;
}

/**
 * A2AServer - WebSocket server for agent-to-agent JSON-RPC communication
 *
 * Handles incoming JSON-RPC requests from other agents, verifies their
 * DID-based authentication, routes to registered method handlers, and
 * sends back responses. Supports middleware, notifications, and heartbeat.
 */
export class A2AServer {
  private config: A2AServerConfig;
  private wss: WebSocketServer | null = null;
  private methodHandlers: Map<string, MethodHandler> = new Map();
  private clients: Map<WebSocket, string> = new Map();
  private middlewares: Middleware[] = [];
  private eventListeners: Map<ServerEvent, Set<ServerEventListener>> = new Map();
  private heartbeatInterval: ReturnType<typeof setInterval> | null = null;
  private heartbeatConfig: HeartbeatConfig | null = null;
  private clientAlive: Map<WebSocket, boolean> = new Map();

  constructor(config: A2AServerConfig) {
    this.config = {
      host: '0.0.0.0',
      ...config,
    };
  }

  /**
   * Add middleware to the processing chain
   */
  use(middleware: Middleware): void {
    this.middlewares.push(middleware);
  }

  /**
   * Register a method handler for incoming RPC calls
   */
  on(method: string, handler: MethodHandler): void {
    this.methodHandlers.set(method, handler);
  }

  /**
   * Register a connection event listener
   */
  onEvent(event: ServerEvent, listener: ServerEventListener): () => void {
    if (!this.eventListeners.has(event)) {
      this.eventListeners.set(event, new Set());
    }
    this.eventListeners.get(event)!.add(listener);
    return () => {
      this.eventListeners.get(event)?.delete(listener);
    };
  }

  /**
   * Enable heartbeat ping/pong for dead client detection
   */
  enableHeartbeat(config?: HeartbeatConfig): void {
    this.heartbeatConfig = {
      intervalMs: config?.intervalMs ?? 30000,
      timeoutMs: config?.timeoutMs ?? 10000,
    };
  }

  /**
   * Send a notification to a specific client (no response expected)
   */
  notify(did: string, method: string, params?: unknown): boolean {
    for (const [ws, clientDid] of this.clients) {
      if (clientDid === did && ws.readyState === WebSocket.OPEN) {
        const notification: JsonRpcRequest = {
          jsonrpc: '2.0',
          method,
          params,
          id: `notify-${Date.now()}`,
        };
        ws.send(JSON.stringify(notification));
        return true;
      }
    }
    return false;
  }

  /**
   * Broadcast a notification to all connected clients
   */
  broadcast(method: string, params?: unknown): number {
    let count = 0;
    const notification: JsonRpcRequest = {
      jsonrpc: '2.0',
      method,
      params,
      id: `broadcast-${Date.now()}`,
    };
    const message = JSON.stringify(notification);

    for (const [ws] of this.clients) {
      if (ws.readyState === WebSocket.OPEN) {
        ws.send(message);
        count++;
      }
    }
    return count;
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
          if (this.heartbeatConfig) {
            this.startHeartbeat();
          }
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
    if (this.heartbeatInterval) {
      clearInterval(this.heartbeatInterval);
      this.heartbeatInterval = null;
    }

    return new Promise((resolve) => {
      if (this.wss) {
        this.clients.forEach((_did, ws) => {
          ws.close();
        });
        this.clients.clear();
        this.clientAlive.clear();

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

  /**
   * Get list of connected client DIDs
   */
  getConnectedClients(): string[] {
    return Array.from(this.clients.values());
  }

  private handleConnection(ws: WebSocket): void {
    this.clientAlive.set(ws, true);

    ws.on('pong', () => {
      this.clientAlive.set(ws, true);
    });

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
      const did = this.clients.get(ws);
      if (did) {
        this.clients.delete(ws);
        this.clientAlive.delete(ws);
        this.emitEvent('client:disconnected', did);
      }
    });

    ws.on('error', (_error) => {
      // Connection error handled by close event
    });
  }

  private async handleRequest(
    request: JsonRpcRequest,
    ws: WebSocket,
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

    // Verify authentication (unless requireAuth is false)
    const requireAuth = this.config.requireAuth !== false;

    if (requireAuth) {
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

      const isValid = await verifyPassport(
        request.auth.from,
        request.auth.signature,
        request,
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
    }

    const fromDID = request.auth?.from ?? 'anonymous';

    // Track client and emit connect event if new
    const wasNew = !this.clients.has(ws);
    this.clients.set(ws, fromDID);
    if (wasNew) {
      this.emitEvent('client:connected', fromDID);
    }

    // Build middleware context
    const ctx: MiddlewareContext = {
      request,
      from: fromDID,
      receivedAt: Date.now(),
      metadata: new Map(),
    };

    // Run through middleware chain then handler
    const handler = this.createFinalHandler();
    const chain = composeMiddleware(this.middlewares, handler);

    return chain(ctx);
  }

  private createFinalHandler(): (ctx: MiddlewareContext) => Promise<JsonRpcResponse> {
    return async (ctx) => {
      const handler = this.methodHandlers.get(ctx.request.method);
      if (!handler) {
        return {
          jsonrpc: '2.0' as const,
          error: {
            code: JsonRpcErrorCode.METHOD_NOT_FOUND,
            message: `Method not found: ${ctx.request.method}`,
          },
          id: ctx.request.id,
        };
      }

      try {
        const result = await handler(ctx.request.params, ctx.from);
        return {
          jsonrpc: '2.0' as const,
          result,
          id: ctx.request.id,
        };
      } catch (error) {
        return {
          jsonrpc: '2.0' as const,
          error: {
            code: JsonRpcErrorCode.INTERNAL_ERROR,
            message: error instanceof Error ? error.message : 'Internal error',
            data: error instanceof Error ? error.stack : undefined,
          },
          id: ctx.request.id,
        };
      }
    };
  }

  private emitEvent(event: ServerEvent, did: string): void {
    const listeners = this.eventListeners.get(event);
    if (listeners) {
      for (const listener of listeners) {
        listener(did);
      }
    }
  }

  private startHeartbeat(): void {
    if (!this.heartbeatConfig) return;

    this.heartbeatInterval = setInterval(() => {
      for (const [ws] of this.clients) {
        if (!this.clientAlive.get(ws)) {
          const did = this.clients.get(ws);
          ws.terminate();
          this.clients.delete(ws);
          this.clientAlive.delete(ws);
          if (did) this.emitEvent('client:disconnected', did);
          continue;
        }
        this.clientAlive.set(ws, false);
        ws.ping();
      }
    }, this.heartbeatConfig.intervalMs);
  }
}
