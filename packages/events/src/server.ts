/**
 * EventServer — WebSocket server that broadcasts AgentEvents to connected clients.
 */

import type { AgentEvent, EventFilter } from '@qualia/types';
import type { AgentEventStream } from './stream';

interface EventServerOptions {
  port: number;
  stream: AgentEventStream;
}

interface ClientState {
  ws: WebSocket;
  filter?: EventFilter;
}

interface SubscribeMessage {
  type: 'subscribe';
  filter: EventFilter;
}

function isSubscribeMessage(data: unknown): data is SubscribeMessage {
  if (typeof data !== 'object' || data === null) return false;
  const msg = data as Record<string, unknown>;
  return msg['type'] === 'subscribe' && typeof msg['filter'] === 'object';
}

function matchesFilter(event: AgentEvent, filter?: EventFilter): boolean {
  if (!filter) return true;
  if (filter.types && filter.types.length > 0) {
    if (!filter.types.includes(event.type)) return false;
  }
  if (filter.sources && filter.sources.length > 0) {
    if (event.source === undefined || !filter.sources.includes(event.source)) return false;
  }
  if (filter.afterSequence !== undefined) {
    if (event.sequence <= filter.afterSequence) return false;
  }
  return true;
}

export class EventServer {
  private readonly _port: number;
  private readonly _stream: AgentEventStream;
  private readonly _clients: Set<ClientState> = new Set();
  private _server: ReturnType<typeof Bun.serve> | null = null;
  private _unsubscribe: (() => void) | null = null;

  constructor(options: EventServerOptions) {
    this._port = options.port;
    this._stream = options.stream;
  }

  /** Start the WebSocket server and begin broadcasting events. */
  async start(): Promise<void> {
    const clients = this._clients;
    const stream = this._stream;

    this._server = Bun.serve({
      port: this._port,
      fetch(req, server) {
        const upgraded = server.upgrade(req, { data: {} });
        if (!upgraded) {
          return new Response('Expected WebSocket', { status: 426 });
        }
        return undefined;
      },
      websocket: {
        open: (ws) => {
          const state: ClientState = { ws: ws as unknown as WebSocket };
          (ws as unknown as Record<string, unknown>)['_clientState'] = state;
          clients.add(state);

          // Send replay events (unfiltered since no filter set yet)
          const replay = stream.getReplay();
          for (const event of replay) {
            ws.send(JSON.stringify(event));
          }
        },
        message: (ws, message) => {
          try {
            const data: unknown = JSON.parse(String(message));
            if (isSubscribeMessage(data)) {
              const state = (ws as unknown as Record<string, unknown>)['_clientState'] as ClientState;
              state.filter = data.filter;

              // Send replay matching the new filter
              const replay = stream.getReplay(data.filter);
              for (const event of replay) {
                ws.send(JSON.stringify(event));
              }
            }
          } catch {
            // Ignore malformed messages
          }
        },
        close: (ws) => {
          const state = (ws as unknown as Record<string, unknown>)['_clientState'] as ClientState | undefined;
          if (state) {
            clients.delete(state);
          }
        },
      },
    });

    // Subscribe to the stream and broadcast to all matching clients
    this._unsubscribe = this._stream.subscribe((event: AgentEvent) => {
      const payload = JSON.stringify(event);
      for (const client of clients) {
        if (matchesFilter(event, client.filter)) {
          try {
            (client.ws as unknown as { send(data: string): void }).send(payload);
          } catch {
            // Client may have disconnected
          }
        }
      }
    });
  }

  /** Stop the server and close all connections. */
  async stop(): Promise<void> {
    if (this._unsubscribe) {
      this._unsubscribe();
      this._unsubscribe = null;
    }
    if (this._server) {
      this._server.stop(true);
      this._server = null;
    }
    this._clients.clear();
  }

  /** Number of currently connected clients. */
  get clientCount(): number {
    return this._clients.size;
  }

  /** The port the server is listening on (useful when using port 0). */
  get port(): number {
    return this._server?.port ?? this._port;
  }
}
