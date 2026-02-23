/**
 * Unit tests for AgentEventStream
 */

import { describe, test, expect } from 'bun:test';
import { AgentEventStream } from '../src/stream';
import type { AgentEvent } from '@qualia/types';

describe('AgentEventStream', () => {
  test('emit creates events with incrementing sequence numbers', () => {
    const stream = new AgentEventStream();
    const e1 = stream.emit('message', { text: 'hello' });
    const e2 = stream.emit('status', { online: true });
    const e3 = stream.emit('error', { code: 500 });

    expect(e1.sequence).toBe(0);
    expect(e2.sequence).toBe(1);
    expect(e3.sequence).toBe(2);
    expect(stream.sequenceNumber).toBe(3);
  });

  test('emit populates all event fields', () => {
    const stream = new AgentEventStream();
    const before = Date.now();
    const event = stream.emit('sensor_data', { temp: 22.5 }, 'agent-1');
    const after = Date.now();

    expect(event.id).toBeDefined();
    expect(event.type).toBe('sensor_data');
    expect(event.data).toEqual({ temp: 22.5 });
    expect(event.source).toBe('agent-1');
    expect(event.timestamp).toBeGreaterThanOrEqual(before);
    expect(event.timestamp).toBeLessThanOrEqual(after);
    expect(event.sequence).toBe(0);
  });

  test('subscribe receives emitted events', () => {
    const stream = new AgentEventStream();
    const received: AgentEvent[] = [];

    stream.subscribe((event) => {
      received.push(event);
    });

    stream.emit('message', 'hello');
    stream.emit('status', 'ok');

    expect(received).toHaveLength(2);
    expect(received[0]!.type).toBe('message');
    expect(received[1]!.type).toBe('status');
  });

  test('filter by event type works', () => {
    const stream = new AgentEventStream();
    const received: AgentEvent[] = [];

    stream.subscribe({ types: ['error'] }, (event) => {
      received.push(event);
    });

    stream.emit('message', 'hello');
    stream.emit('error', 'oops');
    stream.emit('status', 'ok');
    stream.emit('error', 'again');

    expect(received).toHaveLength(2);
    expect(received[0]!.data).toBe('oops');
    expect(received[1]!.data).toBe('again');
  });

  test('filter by source works', () => {
    const stream = new AgentEventStream();
    const received: AgentEvent[] = [];

    stream.subscribe({ sources: ['robot-a'] }, (event) => {
      received.push(event);
    });

    stream.emit('message', 'from-a', 'robot-a');
    stream.emit('message', 'from-b', 'robot-b');
    stream.emit('message', 'from-a-again', 'robot-a');
    stream.emit('message', 'no-source');

    expect(received).toHaveLength(2);
    expect(received[0]!.data).toBe('from-a');
    expect(received[1]!.data).toBe('from-a-again');
  });

  test('unsubscribe stops notifications', () => {
    const stream = new AgentEventStream();
    const received: AgentEvent[] = [];

    const unsub = stream.subscribe((event) => {
      received.push(event);
    });

    stream.emit('message', 'before');
    unsub();
    stream.emit('message', 'after');

    expect(received).toHaveLength(1);
    expect(received[0]!.data).toBe('before');
  });

  test('getReplay returns buffered events', () => {
    const stream = new AgentEventStream({ bufferSize: 100 });

    stream.emit('message', 'one');
    stream.emit('status', 'two');
    stream.emit('error', 'three');

    const replay = stream.getReplay();
    expect(replay).toHaveLength(3);
    expect(replay[0]!.data).toBe('one');
    expect(replay[1]!.data).toBe('two');
    expect(replay[2]!.data).toBe('three');
  });

  test('getReplay respects filter', () => {
    const stream = new AgentEventStream({ bufferSize: 100 });

    stream.emit('message', 'msg1');
    stream.emit('error', 'err1');
    stream.emit('message', 'msg2');
    stream.emit('status', 'stat1');

    const errors = stream.getReplay({ types: ['error'] });
    expect(errors).toHaveLength(1);
    expect(errors[0]!.data).toBe('err1');

    const messages = stream.getReplay({ types: ['message'] });
    expect(messages).toHaveLength(2);
  });

  test('buffer overflow drops oldest events', () => {
    const stream = new AgentEventStream({ bufferSize: 3 });

    stream.emit('message', 'a');
    stream.emit('message', 'b');
    stream.emit('message', 'c');
    stream.emit('message', 'd');
    stream.emit('message', 'e');

    const replay = stream.getReplay();
    expect(replay).toHaveLength(3);
    expect(replay[0]!.data).toBe('c');
    expect(replay[1]!.data).toBe('d');
    expect(replay[2]!.data).toBe('e');
  });

  test('filter by afterSequence works', () => {
    const stream = new AgentEventStream({ bufferSize: 100 });

    stream.emit('message', 'zero');   // seq 0
    stream.emit('message', 'one');    // seq 1
    stream.emit('message', 'two');    // seq 2
    stream.emit('message', 'three');  // seq 3

    const replay = stream.getReplay({ afterSequence: 1 });
    expect(replay).toHaveLength(2);
    expect(replay[0]!.data).toBe('two');
    expect(replay[1]!.data).toBe('three');
  });

  test('multiple subscribers receive the same event', () => {
    const stream = new AgentEventStream();
    const received1: AgentEvent[] = [];
    const received2: AgentEvent[] = [];

    stream.subscribe((event) => { received1.push(event); });
    stream.subscribe((event) => { received2.push(event); });

    stream.emit('message', 'shared');

    expect(received1).toHaveLength(1);
    expect(received2).toHaveLength(1);
    expect(received1[0]!.data).toBe('shared');
    expect(received2[0]!.data).toBe('shared');
  });

  test('each event gets a unique ID', () => {
    const stream = new AgentEventStream();
    const ids = new Set<string>();

    for (let i = 0; i < 50; i++) {
      const event = stream.emit('message', i);
      ids.add(event.id);
    }

    expect(ids.size).toBe(50);
  });
});
