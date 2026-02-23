import type { AgentPosition } from '@qualia/types';

/**
 * Tracks agent positions in-memory for swarm coordination.
 */
export class PositionTracker {
  private positions = new Map<string, AgentPosition>();

  /** Update or add an agent's position. */
  update(position: AgentPosition): void {
    this.positions.set(position.did, position);
  }

  /** Get the latest position for an agent by DID. */
  get(did: string): AgentPosition | null {
    return this.positions.get(did) ?? null;
  }

  /** Get all tracked positions. */
  getAll(): AgentPosition[] {
    return [...this.positions.values()];
  }

  /** Remove an agent from tracking. Returns true if the agent was tracked. */
  remove(did: string): boolean {
    return this.positions.delete(did);
  }

  /**
   * Get nearest agents to a point, sorted by ascending distance.
   * @param x - X coordinate
   * @param y - Y coordinate
   * @param maxCount - Maximum number of results (default: all)
   */
  getNearest(x: number, y: number, maxCount?: number): AgentPosition[] {
    const sorted = [...this.positions.values()].sort((a, b) => {
      const distA = Math.hypot(a.x - x, a.y - y);
      const distB = Math.hypot(b.x - x, b.y - y);
      return distA - distB;
    });

    return maxCount !== undefined ? sorted.slice(0, maxCount) : sorted;
  }

  /**
   * Remove positions older than maxAgeMs.
   * @returns Number of entries removed.
   */
  pruneStale(maxAgeMs: number): number {
    const cutoff = Date.now() - maxAgeMs;
    let removed = 0;

    for (const [did, pos] of this.positions) {
      if (pos.timestamp < cutoff) {
        this.positions.delete(did);
        removed++;
      }
    }

    return removed;
  }
}
