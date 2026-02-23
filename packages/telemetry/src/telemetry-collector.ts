/**
 * TelemetryCollector — buffers metrics and flushes them in batches.
 */

import type { Metric } from '@qualia/types';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface TelemetryCollectorOptions {
  /** Interval in milliseconds between automatic flushes (default: 10 000) */
  flushInterval?: number;
  /** Maximum number of metrics to buffer before forcing a flush (default: 100) */
  maxBatchSize?: number;
  /** Callback invoked on each flush with the buffered metrics */
  onFlush: (metrics: Metric[]) => void;
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

export class TelemetryCollector {
  private readonly _flushInterval: number;
  private readonly _maxBatchSize: number;
  private readonly _onFlush: (metrics: Metric[]) => void;

  private _buffer: Metric[] = [];
  private _timer: ReturnType<typeof setInterval> | null = null;

  constructor(options: TelemetryCollectorOptions) {
    this._flushInterval = options.flushInterval ?? 10_000;
    this._maxBatchSize = options.maxBatchSize ?? 100;
    this._onFlush = options.onFlush;
  }

  // -----------------------------------------------------------------------
  // Public API
  // -----------------------------------------------------------------------

  /**
   * Add a metric to the internal buffer.
   * If the buffer reaches `maxBatchSize`, an automatic flush is triggered.
   */
  record(metric: Metric): void {
    this._buffer.push(metric);

    if (this._buffer.length >= this._maxBatchSize) {
      this.flush();
    }
  }

  /**
   * Immediately flush all buffered metrics by calling `onFlush`, then clear
   * the buffer.  If the buffer is empty the callback is **not** invoked.
   */
  flush(): void {
    if (this._buffer.length === 0) {
      return;
    }

    const batch = this._buffer;
    this._buffer = [];
    this._onFlush(batch);
  }

  /**
   * Start the periodic flush timer.
   */
  start(): void {
    if (this._timer !== null) {
      return; // already running
    }

    this._timer = setInterval(() => {
      this.flush();
    }, this._flushInterval);
  }

  /**
   * Stop the periodic flush timer and flush any remaining metrics.
   */
  stop(): void {
    if (this._timer !== null) {
      clearInterval(this._timer);
      this._timer = null;
    }

    this.flush();
  }

  /**
   * Number of metrics currently buffered.
   */
  get pendingCount(): number {
    return this._buffer.length;
  }
}
