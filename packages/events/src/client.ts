/**
 * EventClient — WebSocket client that receives AgentEvents from an EventServer.
 */

import type { AgentEvent, EventFilter } from '@qualia/types';

interface EventClientOptions {
  url: string;
  filter?: EventFilter;
  autoReconnect?: boolean;
}

const MAX_RECONNECT_DELAY = 30_000;
const INITIAL_RECONNECT_DELAY = 1_000;

export class EventClient {
  private readonly _url: string;
  private readonly _filter: EventFilter | undefined;
  private readonly _autoReconnect: boolean;
  private readonly _listeners: Set<(event: AgentEvent) => void> = new Set();
  private _ws: WebSocket | null = null;
  private _connected = false;
  private _reconnectDelay = INITIAL_RECONNECT_DELAY;
  private _reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private _intentionalClose = false;

  constructor(options: EventClientOptions) {
    this._url = options.url;
    this._filter = options.filter;
    this._autoReconnect = options.autoReconnect ?? true;
  }

  /** Connect to the EventServer. */
  connect(): Promise<void> {
    this._intentionalClose = false;
    return new Promise<void>((resolve, reject) => {
      try {
        this._ws = new WebSocket(this._url);
      } catch (err) {
        reject(err);
        return;
      }

      this._ws.onopen = () => {
        this._connected = true;
        this._reconnectDelay = INITIAL_RECONNECT_DELAY;

        // Send filter if configured
        if (this._filter) {
          this._ws!.send(JSON.stringify({ type: 'subscribe', filter: this._filter }));
        }
        resolve();
      };

      this._ws.onmessage = (msgEvent: MessageEvent) => {
        try {
          const event = JSON.parse(String(msgEvent.data)) as AgentEvent;
          for (const listener of this._listeners) {
            listener(event);
          }
        } catch {
          // Ignore malformed messages
        }
      };

      this._ws.onerror = (err) => {
        if (!this._connected) {
          reject(err);
        }
      };

      this._ws.onclose = () => {
        this._connected = false;
        this._ws = null;

        if (!this._intentionalClose && this._autoReconnect) {
          this._scheduleReconnect();
        }
      };
    });
  }

  /** Disconnect from the EventServer. */
  disconnect(): void {
    this._intentionalClose = true;
    if (this._reconnectTimer !== null) {
      clearTimeout(this._reconnectTimer);
      this._reconnectTimer = null;
    }
    if (this._ws) {
      this._ws.close();
      this._ws = null;
    }
    this._connected = false;
  }

  /** Register an event callback. Returns an unsubscribe function. */
  onEvent(callback: (event: AgentEvent) => void): () => void {
    this._listeners.add(callback);
    return () => {
      this._listeners.delete(callback);
    };
  }

  /** Whether the client is currently connected. */
  get connected(): boolean {
    return this._connected;
  }

  private _scheduleReconnect(): void {
    this._reconnectTimer = setTimeout(() => {
      this._reconnectTimer = null;
      this.connect().catch(() => {
        // Reconnection failed; the onclose handler will schedule again
      });
    }, this._reconnectDelay);

    // Exponential backoff: 1s, 2s, 4s, 8s, ... capped at 30s
    this._reconnectDelay = Math.min(this._reconnectDelay * 2, MAX_RECONNECT_DELAY);
  }
}
