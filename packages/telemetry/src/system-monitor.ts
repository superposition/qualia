/**
 * SystemMonitor — portable system metrics collection
 *
 * Uses only Node.js/Bun built-ins (process, os). No SSH, no external deps.
 */

import { cpuUsage } from 'node:process';
import * as os from 'node:os';

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface CpuSnapshot {
  /** User CPU time percentage (0–100) */
  user: number;
  /** System CPU time percentage (0–100) */
  system: number;
  /** Idle percentage (0–100) */
  idle: number;
}

export interface MemorySnapshot {
  /** Total system memory in bytes */
  total: number;
  /** Used memory in bytes */
  used: number;
  /** Free memory in bytes */
  free: number;
  /** Usage percentage (0–100) */
  percentage: number;
}

export interface SystemSnapshot {
  cpu: CpuSnapshot;
  memory: MemorySnapshot;
  /** Process uptime in seconds */
  uptime: number;
  /** 1, 5, 15 minute load averages (all zeros on non-unix) */
  loadAverage: number[];
  /** Unix timestamp in milliseconds */
  timestamp: number;
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

export class SystemMonitor {
  /**
   * Measure CPU usage over a short sampling interval.
   *
   * Compares `process.cpuUsage()` deltas over 100 ms and converts the
   * microsecond values into percentages of wall-clock time.
   */
  async getCpuUsage(): Promise<CpuSnapshot> {
    const intervalMs = 100;
    const start = cpuUsage();

    await new Promise<void>((resolve) => {
      setTimeout(resolve, intervalMs);
    });

    const delta = cpuUsage(start);

    // cpuUsage returns microseconds; convert to the same scale as the interval
    const totalUs = intervalMs * 1000; // wall-clock interval in µs
    const userPct = (delta.user / totalUs) * 100;
    const systemPct = (delta.system / totalUs) * 100;
    const idlePct = Math.max(0, 100 - userPct - systemPct);

    return {
      user: Math.round(userPct * 100) / 100,
      system: Math.round(systemPct * 100) / 100,
      idle: Math.round(idlePct * 100) / 100,
    };
  }

  /**
   * Return current memory usage derived from `os.totalmem()` / `os.freemem()`.
   */
  getMemoryUsage(): MemorySnapshot {
    const total = os.totalmem();
    const free = os.freemem();
    const used = total - free;
    const percentage = Math.round((used / total) * 10000) / 100;

    return { total, used, free, percentage };
  }

  /**
   * Process uptime in seconds.
   */
  getUptime(): number {
    return process.uptime();
  }

  /**
   * 1-minute, 5-minute, and 15-minute load averages.
   * Returns `[0, 0, 0]` on platforms that do not support load averages.
   */
  getLoadAverage(): number[] {
    return os.loadavg();
  }

  /**
   * Collect all metrics in a single snapshot.
   */
  async snapshot(): Promise<SystemSnapshot> {
    const [cpu] = await Promise.all([this.getCpuUsage()]);

    return {
      cpu,
      memory: this.getMemoryUsage(),
      uptime: this.getUptime(),
      loadAverage: this.getLoadAverage(),
      timestamp: Date.now(),
    };
  }
}
