/**
 * Agent event types for real-time streaming
 */

/** Event types emitted by agents */
export type AgentEventType =
  | 'tool_call'
  | 'tool_result'
  | 'message'
  | 'status'
  | 'sensor_data'
  | 'error'
  | 'navigation'
  | 'discovery';

/** Typed agent event */
export interface AgentEvent<T = unknown> {
  /** Unique event ID */
  id: string;
  /** Event type */
  type: AgentEventType;
  /** Event payload */
  data: T;
  /** Unix timestamp in ms */
  timestamp: number;
  /** Sequence number for ordering */
  sequence: number;
  /** Source agent DID */
  source?: string;
}

/** Filter for subscribing to specific events */
export interface EventFilter {
  /** Event types to include (empty = all) */
  types?: AgentEventType[];
  /** Source agent DIDs to include */
  sources?: string[];
  /** Minimum sequence number */
  afterSequence?: number;
}

/** Event stream configuration */
export interface EventStreamConfig {
  /** Ring buffer size for late-joiner replay */
  bufferSize?: number;
  /** Maximum events per second (backpressure) */
  maxEventsPerSecond?: number;
}
