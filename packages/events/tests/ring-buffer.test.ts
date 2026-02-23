/**
 * Unit tests for RingBuffer
 */

import { describe, test, expect } from 'bun:test';
import { RingBuffer } from '../src/ring-buffer';

describe('RingBuffer', () => {
  test('push and retrieve items in insertion order', () => {
    const buf = new RingBuffer<number>(5);
    buf.push(1);
    buf.push(2);
    buf.push(3);

    expect(buf.toArray()).toEqual([1, 2, 3]);
  });

  test('overflow wraps around and drops oldest', () => {
    const buf = new RingBuffer<number>(3);
    buf.push(1);
    buf.push(2);
    buf.push(3);
    buf.push(4); // overwrites 1
    buf.push(5); // overwrites 2

    expect(buf.toArray()).toEqual([3, 4, 5]);
  });

  test('size never exceeds capacity', () => {
    const buf = new RingBuffer<number>(3);
    for (let i = 0; i < 100; i++) {
      buf.push(i);
      expect(buf.size).toBeLessThanOrEqual(3);
    }
    expect(buf.size).toBe(3);
  });

  test('clear resets state', () => {
    const buf = new RingBuffer<number>(5);
    buf.push(1);
    buf.push(2);
    buf.push(3);
    buf.clear();

    expect(buf.size).toBe(0);
    expect(buf.toArray()).toEqual([]);
  });

  test('empty buffer returns empty array', () => {
    const buf = new RingBuffer<string>(10);

    expect(buf.toArray()).toEqual([]);
    expect(buf.size).toBe(0);
  });

  test('capacity is reported correctly', () => {
    const buf = new RingBuffer<number>(42);
    expect(buf.capacity).toBe(42);
  });

  test('buffer of capacity 1 always holds the latest item', () => {
    const buf = new RingBuffer<string>(1);
    buf.push('a');
    buf.push('b');
    buf.push('c');

    expect(buf.toArray()).toEqual(['c']);
    expect(buf.size).toBe(1);
  });

  test('push after clear works correctly', () => {
    const buf = new RingBuffer<number>(3);
    buf.push(1);
    buf.push(2);
    buf.clear();
    buf.push(10);
    buf.push(20);

    expect(buf.toArray()).toEqual([10, 20]);
    expect(buf.size).toBe(2);
  });

  test('throws on invalid capacity', () => {
    expect(() => new RingBuffer<number>(0)).toThrow();
    expect(() => new RingBuffer<number>(-1)).toThrow();
  });
});
