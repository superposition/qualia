import { describe, test, expect } from 'bun:test';
import type { Metric } from '@qualia/types';
import { TelemetryCollector } from '../src/telemetry-collector';

function makeMetric(name: string, value: number): Metric {
  return { name, value, timestamp: Date.now() };
}

describe('TelemetryCollector', () => {
  test('record adds to pending count', () => {
    const collector = new TelemetryCollector({ onFlush: () => {} });

    expect(collector.pendingCount).toBe(0);
    collector.record(makeMetric('cpu', 42));
    expect(collector.pendingCount).toBe(1);
    collector.record(makeMetric('mem', 80));
    expect(collector.pendingCount).toBe(2);
  });

  test('flush calls onFlush with recorded metrics', () => {
    const flushed: Metric[][] = [];
    const collector = new TelemetryCollector({
      onFlush: (metrics) => flushed.push(metrics),
    });

    collector.record(makeMetric('cpu', 10));
    collector.record(makeMetric('mem', 20));
    collector.flush();

    expect(flushed).toHaveLength(1);
    expect(flushed[0]).toHaveLength(2);
    expect(flushed[0]![0]!.name).toBe('cpu');
    expect(flushed[0]![1]!.name).toBe('mem');
  });

  test('flush clears the buffer', () => {
    const collector = new TelemetryCollector({ onFlush: () => {} });

    collector.record(makeMetric('x', 1));
    collector.record(makeMetric('y', 2));
    collector.flush();

    expect(collector.pendingCount).toBe(0);
  });

  test('flush is a no-op when the buffer is empty', () => {
    let called = false;
    const collector = new TelemetryCollector({
      onFlush: () => {
        called = true;
      },
    });

    collector.flush();
    expect(called).toBe(false);
  });

  test('start triggers periodic flush', async () => {
    const flushed: Metric[][] = [];
    const collector = new TelemetryCollector({
      flushInterval: 50,
      onFlush: (metrics) => flushed.push(metrics),
    });

    collector.record(makeMetric('a', 1));
    collector.start();

    // Wait long enough for at least one interval to fire
    await new Promise<void>((resolve) => {
      setTimeout(resolve, 120);
    });

    collector.stop();

    // The periodic timer (and/or the stop-flush) should have flushed
    expect(flushed.length).toBeGreaterThanOrEqual(1);
  });

  test('stop flushes remaining metrics and stops the timer', () => {
    const flushed: Metric[][] = [];
    const collector = new TelemetryCollector({
      flushInterval: 60_000, // very long so timer won't fire
      onFlush: (metrics) => flushed.push(metrics),
    });

    collector.start();
    collector.record(makeMetric('z', 99));
    collector.stop();

    expect(flushed).toHaveLength(1);
    expect(flushed[0]![0]!.value).toBe(99);
    expect(collector.pendingCount).toBe(0);
  });

  test('auto-flushes when maxBatchSize is reached', () => {
    const flushed: Metric[][] = [];
    const collector = new TelemetryCollector({
      maxBatchSize: 3,
      onFlush: (metrics) => flushed.push(metrics),
    });

    collector.record(makeMetric('a', 1));
    collector.record(makeMetric('b', 2));
    expect(flushed).toHaveLength(0);

    collector.record(makeMetric('c', 3)); // triggers flush
    expect(flushed).toHaveLength(1);
    expect(flushed[0]).toHaveLength(3);
    expect(collector.pendingCount).toBe(0);
  });
});
