/**
 * Frontier detection for exploration planning
 *
 * Detects frontiers (boundaries between explored free space and
 * unexplored unknown space) using BFS-based flood fill.
 */

import type { Frontier } from '@qualia/types';
import type { OccupancyGridMap } from './occupancy-grid';

/** 4-connected neighbor offsets */
const NEIGHBORS: ReadonlyArray<{ dx: number; dy: number }> = [
  { dx: 1, dy: 0 },
  { dx: -1, dy: 0 },
  { dx: 0, dy: 1 },
  { dx: 0, dy: -1 },
];

/** 8-connected neighbor offsets (includes diagonals) */
const NEIGHBORS_8: ReadonlyArray<{ dx: number; dy: number }> = [
  { dx: 1, dy: 0 },
  { dx: -1, dy: 0 },
  { dx: 0, dy: 1 },
  { dx: 0, dy: -1 },
  { dx: 1, dy: 1 },
  { dx: -1, dy: 1 },
  { dx: 1, dy: -1 },
  { dx: -1, dy: -1 },
];

export class FrontierDetector {
  /**
   * Detect all frontiers in the given occupancy grid.
   *
   * A frontier cell is a FREE cell (value 0) adjacent to at least
   * one UNKNOWN cell (value -1). Adjacent frontier cells are grouped
   * into Frontier objects using BFS flood-fill.
   */
  detect(grid: OccupancyGridMap): Frontier[] {
    const visited = new Set<string>();
    const frontiers: Frontier[] = [];
    let frontierCount = 0;

    for (let y = 0; y < grid.height; y++) {
      for (let x = 0; x < grid.width; x++) {
        const key = `${x},${y}`;
        if (visited.has(key)) {
          continue;
        }

        if (!FrontierDetector.isFrontierCell(grid, x, y)) {
          continue;
        }

        // BFS flood-fill to gather all connected frontier cells
        const cells: Array<{ x: number; y: number }> = [];
        const queue: Array<{ x: number; y: number }> = [{ x, y }];
        visited.add(key);

        while (queue.length > 0) {
          const cell = queue.shift()!;
          cells.push(cell);

          for (const n of NEIGHBORS_8) {
            const nx = cell.x + n.dx;
            const ny = cell.y + n.dy;
            const nkey = `${nx},${ny}`;

            if (visited.has(nkey)) {
              continue;
            }

            if (!grid.isInBounds(nx, ny)) {
              continue;
            }

            if (FrontierDetector.isFrontierCell(grid, nx, ny)) {
              visited.add(nkey);
              queue.push({ x: nx, y: ny });
            }
          }
        }

        if (cells.length > 0) {
          const centroid = {
            x: cells.reduce((sum, c) => sum + c.x, 0) / cells.length,
            y: cells.reduce((sum, c) => sum + c.y, 0) / cells.length,
          };

          frontierCount++;
          frontiers.push({
            id: `frontier-${frontierCount}`,
            cells,
            centroid,
            size: cells.length,
          });
        }
      }
    }

    return frontiers;
  }

  /**
   * Check if a cell is a frontier cell.
   * A frontier cell is FREE (value 0) and adjacent (4-connected)
   * to at least one UNKNOWN cell (value -1).
   */
  static isFrontierCell(grid: OccupancyGridMap, x: number, y: number): boolean {
    if (grid.getCell(x, y) !== 0) {
      return false;
    }

    for (const n of NEIGHBORS) {
      const nx = x + n.dx;
      const ny = y + n.dy;
      if (grid.isInBounds(nx, ny) && grid.getCell(nx, ny) === -1) {
        return true;
      }
    }

    return false;
  }
}
