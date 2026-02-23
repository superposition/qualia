import { describe, test, expect, beforeEach } from 'bun:test';
import { FleetAlertManager } from '../src/fleet-alerts';
import type { FleetAlert } from '@qualia/types';

function makeAlert(
  type: FleetAlert['type'],
  message: string,
): FleetAlert {
  return { type, location: { x: 0, y: 0 }, severity: 3, message };
}

describe('FleetAlertManager', () => {
  let manager: FleetAlertManager;

  beforeEach(() => {
    manager = new FleetAlertManager();
  });

  test('raise creates and returns an alert message', () => {
    const alert = makeAlert('obstacle', 'Rock ahead');
    const msg = manager.raise(alert, 'did:key:zAlice');

    expect(msg.type).toBe('alert');
    expect(msg.from).toBe('did:key:zAlice');
    expect(msg.data.message).toBe('Rock ahead');
    expect(msg.data.type).toBe('obstacle');
    expect(msg.timestamp).toBeGreaterThan(0);
  });

  test('getRecent returns all alerts when no maxAge given', () => {
    manager.raise(makeAlert('obstacle', 'a'), 'did:key:z1');
    manager.raise(makeAlert('danger', 'b'), 'did:key:z2');

    const recent = manager.getRecent();
    expect(recent).toHaveLength(2);
  });

  test('getRecent filters by age', async () => {
    manager.raise(makeAlert('obstacle', 'old'), 'did:key:z1');

    // Wait a small amount so the first alert ages
    await new Promise((resolve) => setTimeout(resolve, 50));

    manager.raise(makeAlert('danger', 'new'), 'did:key:z2');

    const recent = manager.getRecent(30);
    expect(recent).toHaveLength(1);
    expect(recent[0]!.data.message).toBe('new');
  });

  test('getByType filters by alert type', () => {
    manager.raise(makeAlert('obstacle', 'a'), 'did:key:z1');
    manager.raise(makeAlert('danger', 'b'), 'did:key:z2');
    manager.raise(makeAlert('obstacle', 'c'), 'did:key:z3');
    manager.raise(makeAlert('low_battery', 'd'), 'did:key:z4');

    const obstacles = manager.getByType('obstacle');
    expect(obstacles).toHaveLength(2);
    expect(obstacles.every((a) => a.data.type === 'obstacle')).toBe(true);
  });

  test('clear removes all alerts', () => {
    manager.raise(makeAlert('obstacle', 'a'), 'did:key:z1');
    manager.raise(makeAlert('danger', 'b'), 'did:key:z2');

    manager.clear();

    expect(manager.getRecent()).toHaveLength(0);
  });

  test('getByType returns empty array for unmatched type', () => {
    manager.raise(makeAlert('obstacle', 'a'), 'did:key:z1');

    expect(manager.getByType('stuck')).toHaveLength(0);
  });
});
