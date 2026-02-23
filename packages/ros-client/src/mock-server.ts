/**
 * MockROSBridge - A lightweight rosbridge-protocol-compatible WebSocket
 * server for testing purposes.
 */

import { WebSocketServer, WebSocket } from 'ws';

export class MockROSBridge {
  private readonly _port: number;
  private _wss: WebSocketServer | null = null;
  private readonly _clients = new Set<WebSocket>();

  /** topic -> callback invoked when a client subscribes */
  private readonly _subscribeHooks = new Map<string, () => void>();
  /** service -> handler returning result values */
  private readonly _serviceHandlers = new Map<string, (args: unknown) => unknown>();

  constructor(port: number) {
    this._port = port;
  }

  /** Start the WebSocket server. Resolves once listening. */
  start(): Promise<void> {
    return new Promise<void>((resolve, reject) => {
      this._wss = new WebSocketServer({ port: this._port });

      this._wss.on('listening', () => {
        resolve();
      });

      this._wss.on('error', (err) => {
        reject(err);
      });

      this._wss.on('connection', (ws) => {
        this._clients.add(ws);

        ws.on('message', (data: Buffer | string) => {
          this._handleMessage(ws, data);
        });

        ws.on('close', () => {
          this._clients.delete(ws);
        });
      });
    });
  }

  /** Stop the server and close all client connections. */
  stop(): Promise<void> {
    return new Promise<void>((resolve) => {
      if (!this._wss) {
        resolve();
        return;
      }

      for (const client of this._clients) {
        client.terminate();
      }
      this._clients.clear();

      this._wss.close(() => {
        this._wss = null;
        resolve();
      });

      // Safety: resolve after a short delay if the callback doesn't fire (Bun compat)
      setTimeout(() => {
        if (this._wss) {
          this._wss = null;
          resolve();
        }
      }, 500);
    });
  }

  /** Register a hook that fires when a client subscribes to a topic. */
  onSubscribe(topic: string, callback: () => void): void {
    this._subscribeHooks.set(topic, callback);
  }

  /** Publish a message to all connected clients. */
  publish(topic: string, msg: unknown): void {
    const payload = JSON.stringify({ op: 'publish', topic, msg });
    for (const client of this._clients) {
      if (client.readyState === WebSocket.OPEN) {
        client.send(payload);
      }
    }
  }

  /** Register a handler for a service. The handler receives args and returns values. */
  onServiceCall(service: string, handler: (args: unknown) => unknown): void {
    this._serviceHandlers.set(service, handler);
  }

  /** Number of currently connected clients. */
  get clientCount(): number {
    return this._clients.size;
  }

  // -----------------------------------------------------------------------
  // Internals
  // -----------------------------------------------------------------------

  private _handleMessage(ws: WebSocket, data: Buffer | string): void {
    let msg: Record<string, unknown>;
    try {
      msg = JSON.parse(typeof data === 'string' ? data : data.toString()) as Record<string, unknown>;
    } catch {
      return;
    }

    const op = msg['op'] as string | undefined;

    switch (op) {
      case 'subscribe': {
        const topic = msg['topic'] as string;
        const hook = this._subscribeHooks.get(topic);
        if (hook) hook();
        break;
      }
      case 'call_service': {
        const service = msg['service'] as string;
        const id = msg['id'] as string | undefined;
        const handler = this._serviceHandlers.get(service);

        if (handler) {
          try {
            const values = handler(msg['args']);
            ws.send(JSON.stringify({
              op: 'service_response',
              service,
              id,
              values,
              result: true,
            }));
          } catch {
            ws.send(JSON.stringify({
              op: 'service_response',
              service,
              id,
              values: null,
              result: false,
            }));
          }
        } else {
          ws.send(JSON.stringify({
            op: 'service_response',
            service,
            id,
            values: null,
            result: false,
          }));
        }
        break;
      }
      default:
        // Ignore publish, advertise, unadvertise, unsubscribe, etc.
        break;
    }
  }
}
