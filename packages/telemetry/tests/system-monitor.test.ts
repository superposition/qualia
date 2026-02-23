import { describe, test, expect } from 'bun:test';
import { SystemMonitor } from '../src/system-monitor';

describe('SystemMonitor', () => {
  const monitor = new SystemMonitor();

  test('getMemoryUsage returns positive numbers', () => {
    const mem = monitor.getMemoryUsage();

    expect(mem.total).toBeGreaterThan(0);
    expect(mem.used).toBeGreaterThan(0);
    expect(mem.free).toBeGreaterThan(0);
    expect(mem.percentage).toBeGreaterThan(0);
    expect(mem.percentage).toBeLessThanOrEqual(100);
  });

  test('getUptime returns a positive number', () => {
    const uptime = monitor.getUptime();

    expect(uptime).toBeGreaterThan(0);
  });

  test('getLoadAverage returns an array of 3 numbers', () => {
    const avg = monitor.getLoadAverage();

    expect(avg).toHaveLength(3);
    expect(typeof avg[0]).toBe('number');
    expect(typeof avg[1]).toBe('number');
    expect(typeof avg[2]).toBe('number');
  });

  test('getCpuUsage returns values in a reasonable range', async () => {
    const cpu = await monitor.getCpuUsage();

    expect(cpu.user).toBeGreaterThanOrEqual(0);
    expect(cpu.system).toBeGreaterThanOrEqual(0);
    expect(cpu.idle).toBeGreaterThanOrEqual(0);
    // user + system + idle should be ~100
    const total = cpu.user + cpu.system + cpu.idle;
    expect(total).toBeCloseTo(100, 0);
  });

  test('snapshot returns all fields with correct types', async () => {
    const snap = await monitor.snapshot();

    // CPU
    expect(snap.cpu).toBeDefined();
    expect(typeof snap.cpu.user).toBe('number');
    expect(typeof snap.cpu.system).toBe('number');
    expect(typeof snap.cpu.idle).toBe('number');

    // Memory
    expect(snap.memory.total).toBeGreaterThan(0);

    // Uptime
    expect(snap.uptime).toBeGreaterThan(0);

    // Load average
    expect(snap.loadAverage).toHaveLength(3);

    // Timestamp
    expect(snap.timestamp).toBeGreaterThan(0);
    expect(snap.timestamp).toBeLessThanOrEqual(Date.now());
  });

  test('memory total equals used + free', () => {
    const mem = monitor.getMemoryUsage();

    expect(mem.total).toBe(mem.used + mem.free);
  });
});
