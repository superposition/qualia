import { describe, test, expect } from 'bun:test';
import { PositionTracker } from '../src/position-tracker';
import type { AgentPosition } from '@qualia/types';

function makePosition(
  did: string,
  x: number,
  y: number,
  timestamp?: number,
): AgentPosition {
  return { did, x, y, theta: 0, timestamp: timestamp ?? Date.now() };
}

describe('PositionTracker', () => {
  test('add and get a position', () => {
    const tracker = new PositionTracker();
    const pos = makePosition('did:key:zAlice', 1, 2);

    tracker.update(pos);
    const result = tracker.get('did:key:zAlice');

    expect(result).not.toBeNull();
    expect(result!.x).toBe(1);
    expect(result!.y).toBe(2);
  });

  test('get returns null for unknown agent', () => {
    const tracker = new PositionTracker();
    expect(tracker.get('did:key:zUnknown')).toBeNull();
  });

  test('update overwrites previous position', () => {
    const tracker = new PositionTracker();
    tracker.update(makePosition('did:key:zAlice', 1, 2));
    tracker.update(makePosition('did:key:zAlice', 5, 10));

    const result = tracker.get('did:key:zAlice');
    expect(result!.x).toBe(5);
    expect(result!.y).toBe(10);
  });

  test('getAll returns all tracked positions', () => {
    const tracker = new PositionTracker();
    tracker.update(makePosition('did:key:zAlice', 0, 0));
    tracker.update(makePosition('did:key:zBob', 3, 4));
    tracker.update(makePosition('did:key:zCarol', 6, 8));

    const all = tracker.getAll();
    expect(all).toHaveLength(3);
  });

  test('remove deletes position and returns true', () => {
    const tracker = new PositionTracker();
    tracker.update(makePosition('did:key:zAlice', 0, 0));

    expect(tracker.remove('did:key:zAlice')).toBe(true);
    expect(tracker.get('did:key:zAlice')).toBeNull();
  });

  test('remove returns false for unknown agent', () => {
    const tracker = new PositionTracker();
    expect(tracker.remove('did:key:zUnknown')).toBe(false);
  });

  test('getNearest returns agents sorted by distance', () => {
    const tracker = new PositionTracker();
    tracker.update(makePosition('did:key:zFar', 10, 10));
    tracker.update(makePosition('did:key:zMid', 3, 4));
    tracker.update(makePosition('did:key:zNear', 1, 0));

    const nearest = tracker.getNearest(0, 0);

    expect(nearest).toHaveLength(3);
    expect(nearest[0]!.did).toBe('did:key:zNear');
    expect(nearest[1]!.did).toBe('did:key:zMid');
    expect(nearest[2]!.did).toBe('did:key:zFar');
  });

  test('getNearest respects maxCount', () => {
    const tracker = new PositionTracker();
    tracker.update(makePosition('did:key:zA', 1, 0));
    tracker.update(makePosition('did:key:zB', 2, 0));
    tracker.update(makePosition('did:key:zC', 3, 0));

    const nearest = tracker.getNearest(0, 0, 2);
    expect(nearest).toHaveLength(2);
    expect(nearest[0]!.did).toBe('did:key:zA');
    expect(nearest[1]!.did).toBe('did:key:zB');
  });

  test('pruneStale removes old entries and returns count', () => {
    const tracker = new PositionTracker();
    const old = Date.now() - 60_000;
    const recent = Date.now();

    tracker.update(makePosition('did:key:zOld', 0, 0, old));
    tracker.update(makePosition('did:key:zRecent', 1, 1, recent));

    const removed = tracker.pruneStale(30_000);

    expect(removed).toBe(1);
    expect(tracker.get('did:key:zOld')).toBeNull();
    expect(tracker.get('did:key:zRecent')).not.toBeNull();
  });
});
