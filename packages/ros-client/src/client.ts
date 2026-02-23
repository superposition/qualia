/**
 * ROSBridgeClient - WebSocket client for the rosbridge protocol.
 *
 * Connects to a rosbridge_server over WebSocket and provides typed
 * pub/sub, service calls, and parameter get/set operations.
 */

import { WebSocket } from 'ws';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface ROSBridgeClientOptions {
  /** WebSocket URL of the rosbridge server (e.g. ws://localhost:9090) */
  url: string;
  /** Enable automatic reconnection on disconnect (default: false) */
  autoReconnect?: boolean;
  /** Initial delay between reconnection attempts in ms (default: 1000) */
  reconnectDelayMs?: number;
  /** Maximum number of reconnection attempts before giving up (default: 10) */
  maxReconnectAttempts?: number;
}

interface PendingServiceCall {
  resolve: (value: unknown) => void;
  reject: (reason: Error) => void;
  timer: ReturnType<typeof setTimeout>;
}

interface AdvertisedTopic<T> {
  publish: (msg: T) => void;
  unadvertise: () => void;
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

export class ROSBridgeClient {
  private readonly _url: string;
  private readonly _autoReconnect: boolean;
  private readonly _reconnectDelayMs: number;
  private readonly _maxReconnectAttempts: number;

  private _ws: WebSocket | null = null;
  private _connected = false;
  private _reconnectAttempts = 0;
  private _reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private _intentionalClose = false;

  /** topic -> Set of callbacks */
  private readonly _subscriptions = new Map<string, Set<(msg: unknown) => void>>();
  /** id -> pending service call */
  private readonly _pendingCalls = new Map<string, PendingServiceCall>();
  /** Monotonic counter for unique message IDs */
  private _idCounter = 0;

  constructor(options: ROSBridgeClientOptions) {
    this._url = options.url;
    this._autoReconnect = options.autoReconnect ?? false;
    this._reconnectDelayMs = options.reconnectDelayMs ?? 1000;
    this._maxReconnectAttempts = options.maxReconnectAttempts ?? 10;
  }

  // -----------------------------------------------------------------------
  // Connection lifecycle
  // -----------------------------------------------------------------------

  /** Connect to the rosbridge server. Resolves once the WebSocket is open. */
  connect(): Promise<void> {
    return new Promise<void>((resolve, reject) => {
      this._intentionalClose = false;
      this._ws = new WebSocket(this._url);

      this._ws.on('open', () => {
        this._connected = true;
        this._reconnectAttempts = 0;

        // Re-subscribe to all active topics
        for (const [topic] of this._subscriptions) {
          this._send({ op: 'subscribe', topic });
        }

        resolve();
      });

      this._ws.on('message', (data: Buffer | string) => {
        this._handleMessage(data);
      });

      this._ws.on('close', () => {
        this._connected = false;
        if (!this._intentionalClose && this._autoReconnect) {
          this._scheduleReconnect();
        }
      });

      this._ws.on('error', (err: Error) => {
        if (!this._connected) {
          reject(err);
        }
      });
    });
  }

  /** Close the WebSocket connection. */
  disconnect(): void {
    this._intentionalClose = true;
    if (this._reconnectTimer !== null) {
      clearTimeout(this._reconnectTimer);
      this._reconnectTimer = null;
    }
    // Reject all pending service calls
    for (const [id, pending] of this._pendingCalls) {
      clearTimeout(pending.timer);
      pending.reject(new Error('Client disconnected'));
      this._pendingCalls.delete(id);
    }
    if (this._ws) {
      this._ws.terminate();
      this._ws = null;
    }
    this._connected = false;
  }

  /** Whether the client currently has an open WebSocket connection. */
  get connected(): boolean {
    return this._connected;
  }

  // -----------------------------------------------------------------------
  // Pub / Sub
  // -----------------------------------------------------------------------

  /**
   * Subscribe to a ROS topic. Returns an unsubscribe function.
   *
   * When the first subscriber is added the client sends a rosbridge
   * `subscribe` op. When the last subscriber unsubscribes, the client sends
   * an `unsubscribe` op.
   */
  subscribe<T>(topic: string, type: string, callback: (msg: T) => void): () => void {
    let callbacks = this._subscriptions.get(topic);
    if (!callbacks) {
      callbacks = new Set();
      this._subscriptions.set(topic, callbacks);
      this._send({ op: 'subscribe', topic, type });
    }

    callbacks.add(callback as (msg: unknown) => void);

    return () => {
      const cbs = this._subscriptions.get(topic);
      if (cbs) {
        cbs.delete(callback as (msg: unknown) => void);
        if (cbs.size === 0) {
          this._subscriptions.delete(topic);
          this._send({ op: 'unsubscribe', topic });
        }
      }
    };
  }

  /**
   * Advertise a topic and return a handle for publishing and un-advertising.
   */
  advertise<T>(topic: string, type: string): AdvertisedTopic<T> {
    this._send({ op: 'advertise', topic, type });

    return {
      publish: (msg: T) => {
        this._send({ op: 'publish', topic, msg });
      },
      unadvertise: () => {
        this._send({ op: 'unadvertise', topic });
      },
    };
  }

  // -----------------------------------------------------------------------
  // Service calls
  // -----------------------------------------------------------------------

  /** Call a ROS service. Resolves with the service response values. */
  callService<TReq, TRes>(
    service: string,
    type: string,
    request: TReq,
    timeoutMs = 10_000,
  ): Promise<TRes> {
    return new Promise<TRes>((resolve, reject) => {
      if (!this._ws || !this._connected) {
        reject(new Error('Not connected to rosbridge'));
        return;
      }

      const id = this._nextId();

      const timer = setTimeout(() => {
        this._pendingCalls.delete(id);
        reject(new Error(`Service call to ${service} timed out`));
      }, timeoutMs);

      this._pendingCalls.set(id, {
        resolve: resolve as (v: unknown) => void,
        reject,
        timer,
      });

      this._send({ op: 'call_service', service, type, args: request, id });
    });
  }

  // -----------------------------------------------------------------------
  // Parameters
  // -----------------------------------------------------------------------

  /** Get a ROS parameter value. */
  getParam<T>(name: string): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      if (!this._ws || !this._connected) {
        reject(new Error('Not connected to rosbridge'));
        return;
      }

      const id = this._nextId();

      const timer = setTimeout(() => {
        this._pendingCalls.delete(id);
        reject(new Error(`getParam ${name} timed out`));
      }, 10_000);

      this._pendingCalls.set(id, {
        resolve: resolve as (v: unknown) => void,
        reject,
        timer,
      });

      this._send({ op: 'call_service', service: '/rosapi/get_param', args: { name }, id });
    });
  }

  /** Set a ROS parameter value. */
  setParam<T>(name: string, value: T): Promise<void> {
    return new Promise<void>((resolve, reject) => {
      if (!this._ws || !this._connected) {
        reject(new Error('Not connected to rosbridge'));
        return;
      }

      const id = this._nextId();

      const timer = setTimeout(() => {
        this._pendingCalls.delete(id);
        reject(new Error(`setParam ${name} timed out`));
      }, 10_000);

      this._pendingCalls.set(id, {
        resolve: resolve as (v: unknown) => void,
        reject,
        timer,
      });

      this._send({
        op: 'call_service',
        service: '/rosapi/set_param',
        args: { name, value: JSON.stringify(value) },
        id,
      });
    });
  }

  // -----------------------------------------------------------------------
  // Internals
  // -----------------------------------------------------------------------

  private _nextId(): string {
    this._idCounter += 1;
    return `ros_client_${this._idCounter}_${Date.now()}`;
  }

  private _send(msg: Record<string, unknown>): void {
    if (this._ws && this._connected) {
      this._ws.send(JSON.stringify(msg));
    }
  }

  private _handleMessage(data: Buffer | string): void {
    let msg: Record<string, unknown>;
    try {
      msg = JSON.parse(typeof data === 'string' ? data : data.toString()) as Record<string, unknown>;
    } catch {
      return; // ignore malformed JSON
    }

    const op = msg['op'] as string | undefined;

    switch (op) {
      case 'publish': {
        const topic = msg['topic'] as string;
        const callbacks = this._subscriptions.get(topic);
        if (callbacks) {
          const payload = msg['msg'];
          for (const cb of callbacks) {
            cb(payload);
          }
        }
        break;
      }
      case 'service_response': {
        const id = msg['id'] as string | undefined;
        if (id) {
          const pending = this._pendingCalls.get(id);
          if (pending) {
            clearTimeout(pending.timer);
            this._pendingCalls.delete(id);
            if (msg['result'] === true || msg['result'] === undefined) {
              pending.resolve(msg['values']);
            } else {
              pending.reject(new Error(`Service call failed: ${String(msg['values'] ?? 'unknown error')}`));
            }
          }
        }
        break;
      }
      default:
        // Ignore unknown ops
        break;
    }
  }

  private _scheduleReconnect(): void {
    if (this._reconnectAttempts >= this._maxReconnectAttempts) {
      return;
    }

    const delay = this._reconnectDelayMs * Math.pow(2, this._reconnectAttempts);
    this._reconnectAttempts += 1;

    this._reconnectTimer = setTimeout(() => {
      this._reconnectTimer = null;
      this.connect().catch(() => {
        // If reconnect fails, the close handler will schedule another attempt
      });
    }, delay);
  }
}
