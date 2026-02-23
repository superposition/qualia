/**
 * AgentEventStream — produces typed events with replay support.
 */

import type {
  AgentEvent,
  AgentEventType,
  EventFilter,
  EventStreamConfig,
} from '@qualia/types';
import { RingBuffer } from './ring-buffer';

const DEFAULT_BUFFER_SIZE = 1000;

/** Generate a unique event ID. */
function generateEventId(): string {
  return crypto.randomUUID();
}

/** Check whether an event matches a filter. */
function matchesFilter(event: AgentEvent, filter: EventFilter): boolean {
  if (filter.types && filter.types.length > 0) {
    if (!filter.types.includes(event.type)) {
      return false;
    }
  }
  if (filter.sources && filter.sources.length > 0) {
    if (event.source === undefined || !filter.sources.includes(event.source)) {
      return false;
    }
  }
  if (filter.afterSequence !== undefined) {
    if (event.sequence <= filter.afterSequence) {
      return false;
    }
  }
  return true;
}

type Listener = {
  filter: EventFilter | undefined;
  callback: (event: AgentEvent) => void;
};

export class AgentEventStream {
  private _sequence = 0;
  private readonly _buffer: RingBuffer<AgentEvent>;
  private readonly _listeners: Set<Listener> = new Set();

  constructor(config?: EventStreamConfig) {
    const bufferSize = config?.bufferSize ?? DEFAULT_BUFFER_SIZE;
    this._buffer = new RingBuffer<AgentEvent>(bufferSize);
  }

  /** Emit a new event, store it in the ring buffer, and notify listeners. */
  emit<T>(type: AgentEventType, data: T, source?: string): AgentEvent<T> {
    const base = {
      id: generateEventId(),
      type,
      data,
      timestamp: Date.now(),
      sequence: this._sequence++,
    };
    const event: AgentEvent<T> = source !== undefined
      ? { ...base, source }
      : base;

    this._buffer.push(event as AgentEvent);

    for (const listener of this._listeners) {
      if (!listener.filter || matchesFilter(event as AgentEvent, listener.filter)) {
        listener.callback(event as AgentEvent);
      }
    }

    return event;
  }

  /** Subscribe to events. Returns an unsubscribe function. */
  subscribe(
    filterOrCallback: EventFilter | ((event: AgentEvent) => void),
    maybeCallback?: (event: AgentEvent) => void,
  ): () => void {
    let filter: EventFilter | undefined;
    let callback: (event: AgentEvent) => void;

    if (typeof filterOrCallback === 'function') {
      filter = undefined;
      callback = filterOrCallback;
    } else {
      filter = filterOrCallback;
      callback = maybeCallback!;
    }

    const listener: Listener = { filter, callback };
    this._listeners.add(listener);

    return () => {
      this._listeners.delete(listener);
    };
  }

  /** Get buffered events matching the optional filter. */
  getReplay(filter?: EventFilter): AgentEvent[] {
    const all = this._buffer.toArray();
    if (!filter) {
      return all;
    }
    return all.filter((event) => matchesFilter(event, filter));
  }

  /** Current sequence number (next event will have this sequence). */
  get sequenceNumber(): number {
    return this._sequence;
  }
}
