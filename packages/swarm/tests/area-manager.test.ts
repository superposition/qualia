import { describe, test, expect } from 'bun:test';
import { AreaManager } from '../src/area-manager';
import type { AreaClaim } from '@qualia/types';

function makeClaim(
  id: string,
  x: number,
  y: number,
  radius: number,
  expiresAt?: number,
): AreaClaim {
  return {
    id,
    agentDid: `did:key:z${id}`,
    center: { x, y },
    radius,
    expiresAt: expiresAt ?? Date.now() + 60_000,
  };
}

describe('AreaManager', () => {
  test('claim succeeds for empty area', () => {
    const manager = new AreaManager();
    const claim = makeClaim('c1', 0, 0, 5);

    expect(manager.claim(claim)).toBe(true);
    expect(manager.getClaims()).toHaveLength(1);
  });

  test('claim fails for overlapping area', () => {
    const manager = new AreaManager();
    manager.claim(makeClaim('c1', 0, 0, 5));

    // Center at (3, 0) with radius 5 overlaps with (0, 0) r=5: distance 3 < 5 + 5
    const overlapping = makeClaim('c2', 3, 0, 5);
    expect(manager.claim(overlapping)).toBe(false);
  });

  test('claim succeeds for non-overlapping area', () => {
    const manager = new AreaManager();
    manager.claim(makeClaim('c1', 0, 0, 5));

    // Center at (20, 0) with radius 5: distance 20 >= 5 + 5
    const distant = makeClaim('c2', 20, 0, 5);
    expect(manager.claim(distant)).toBe(true);
  });

  test('release frees area for new claims', () => {
    const manager = new AreaManager();
    manager.claim(makeClaim('c1', 0, 0, 5));

    expect(manager.release('c1')).toBe(true);
    expect(manager.getClaims()).toHaveLength(0);

    // Now overlapping claim should succeed
    const overlapping = makeClaim('c2', 3, 0, 5);
    expect(manager.claim(overlapping)).toBe(true);
  });

  test('release returns false for non-existent claim', () => {
    const manager = new AreaManager();
    expect(manager.release('no-such-claim')).toBe(false);
  });

  test('getClaimAt finds covering claim', () => {
    const manager = new AreaManager();
    manager.claim(makeClaim('c1', 10, 10, 5));

    const found = manager.getClaimAt(12, 10);
    expect(found).not.toBeNull();
    expect(found!.id).toBe('c1');
  });

  test('getClaimAt returns null for uncovered point', () => {
    const manager = new AreaManager();
    manager.claim(makeClaim('c1', 10, 10, 5));

    expect(manager.getClaimAt(100, 100)).toBeNull();
  });

  test('isAvailable returns true for unclaimed area', () => {
    const manager = new AreaManager();
    manager.claim(makeClaim('c1', 0, 0, 5));

    expect(manager.isAvailable({ x: 50, y: 50 }, 5)).toBe(true);
  });

  test('isAvailable returns false for overlapping area', () => {
    const manager = new AreaManager();
    manager.claim(makeClaim('c1', 0, 0, 5));

    expect(manager.isAvailable({ x: 3, y: 0 }, 5)).toBe(false);
  });

  test('pruneExpired removes expired claims and returns count', () => {
    const manager = new AreaManager();
    const expired = Date.now() - 1000;
    const future = Date.now() + 60_000;

    // Directly add claims (one expired, one active)
    manager.claim(makeClaim('active', 0, 0, 5, future));
    manager.claim(makeClaim('expired', 50, 50, 5, expired));

    const removed = manager.pruneExpired();

    expect(removed).toBe(1);
    expect(manager.getClaims()).toHaveLength(1);
    expect(manager.getClaims()[0]!.id).toBe('active');
  });
});
