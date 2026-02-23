/**
 * Occupancy grid map for spatial representation
 *
 * Stores a 2D grid of occupancy values and provides methods for
 * coordinate conversion, lidar-based updates, and export.
 */

import type { OccupancyGrid } from '@qualia/types';

/** Configuration for creating an occupancy grid */
export interface OccupancyGridConfig {
  /** Grid width in cells */
  width: number;
  /** Grid height in cells */
  height: number;
  /** Meters per cell */
  resolution: number;
  /** World coordinates of grid origin (bottom-left) */
  origin?: { x: number; y: number };
}

export class OccupancyGridMap {
  readonly width: number;
  readonly height: number;
  readonly resolution: number;
  readonly origin: { x: number; y: number };

  /** Flat array of occupancy values: -1 = unknown, 0 = free, 100 = occupied */
  private readonly data: number[];

  constructor(config: OccupancyGridConfig) {
    this.width = config.width;
    this.height = config.height;
    this.resolution = config.resolution;
    this.origin = config.origin ?? { x: 0, y: 0 };
    this.data = new Array<number>(this.width * this.height).fill(-1);
  }

  /** Get occupancy value at grid coordinates. Returns -1 for out-of-bounds. */
  getCell(x: number, y: number): number {
    if (!this.isInBounds(x, y)) {
      return -1;
    }
    return this.data[y * this.width + x]!;
  }

  /** Set occupancy value at grid coordinates */
  setCell(x: number, y: number, value: number): void {
    if (!this.isInBounds(x, y)) {
      return;
    }
    this.data[y * this.width + x] = value;
  }

  /** Convert world coordinates to grid coordinates */
  worldToGrid(wx: number, wy: number): { x: number; y: number } {
    return {
      x: Math.floor((wx - this.origin.x) / this.resolution),
      y: Math.floor((wy - this.origin.y) / this.resolution),
    };
  }

  /** Convert grid coordinates to world coordinates (center of cell) */
  gridToWorld(gx: number, gy: number): { x: number; y: number } {
    return {
      x: this.origin.x + (gx + 0.5) * this.resolution,
      y: this.origin.y + (gy + 0.5) * this.resolution,
    };
  }

  /** Check if grid coordinates are within bounds */
  isInBounds(x: number, y: number): boolean {
    return x >= 0 && x < this.width && y >= 0 && y < this.height;
  }

  /**
   * Update the grid from a lidar scan using Bresenham's line algorithm.
   *
   * For each valid range reading:
   * - Mark cells along the ray as free (0)
   * - Mark the endpoint cell as occupied (100)
   *
   * Invalid ranges (NaN, Infinity, > rangeMax) are skipped.
   */
  updateFromLidar(
    robotX: number,
    robotY: number,
    robotTheta: number,
    ranges: number[],
    angleMin: number,
    angleIncrement: number,
    rangeMax: number,
  ): void {
    const robotGrid = this.worldToGrid(robotX, robotY);

    for (let i = 0; i < ranges.length; i++) {
      const range = ranges[i]!;

      // Skip invalid ranges
      if (!Number.isFinite(range) || range > rangeMax || range <= 0) {
        continue;
      }

      const angle = robotTheta + angleMin + i * angleIncrement;

      // Compute the endpoint in world coordinates
      const endX = robotX + range * Math.cos(angle);
      const endY = robotY + range * Math.sin(angle);
      const endGrid = this.worldToGrid(endX, endY);

      // Trace the ray using Bresenham's line algorithm
      this.bresenham(robotGrid.x, robotGrid.y, endGrid.x, endGrid.y, (x, y, isEnd) => {
        if (this.isInBounds(x, y)) {
          if (isEnd) {
            this.data[y * this.width + x] = 100;
          } else {
            this.data[y * this.width + x] = 0;
          }
        }
      });
    }
  }

  /** Export as OccupancyGrid type from @qualia/types */
  toGrid(): OccupancyGrid {
    return {
      width: this.width,
      height: this.height,
      resolution: this.resolution,
      origin: { ...this.origin },
      data: [...this.data],
    };
  }

  /**
   * Visual debug output of the grid.
   * '#' = occupied, '.' = free, '?' = unknown
   * Rows are printed top-to-bottom (highest y first).
   */
  toAscii(robotGridX?: number, robotGridY?: number): string {
    const lines: string[] = [];
    for (let y = this.height - 1; y >= 0; y--) {
      let row = '';
      for (let x = 0; x < this.width; x++) {
        if (robotGridX !== undefined && robotGridY !== undefined && x === robotGridX && y === robotGridY) {
          row += 'R';
          continue;
        }
        const val = this.data[y * this.width + x]!;
        if (val === -1) {
          row += '?';
        } else if (val === 0) {
          row += '.';
        } else {
          row += '#';
        }
      }
      lines.push(row);
    }
    return lines.join('\n');
  }

  /**
   * Bresenham's line algorithm.
   * Calls the callback for each cell along the line.
   * The callback receives (x, y, isEndpoint).
   */
  private bresenham(
    x0: number,
    y0: number,
    x1: number,
    y1: number,
    callback: (x: number, y: number, isEnd: boolean) => void,
  ): void {
    const dx = Math.abs(x1 - x0);
    const dy = Math.abs(y1 - y0);
    const sx = x0 < x1 ? 1 : -1;
    const sy = y0 < y1 ? 1 : -1;
    let err = dx - dy;

    let cx = x0;
    let cy = y0;

    for (;;) {
      const isEnd = cx === x1 && cy === y1;
      callback(cx, cy, isEnd);

      if (isEnd) {
        break;
      }

      const e2 = 2 * err;
      if (e2 > -dy) {
        err -= dy;
        cx += sx;
      }
      if (e2 < dx) {
        err += dx;
        cy += sy;
      }
    }
  }
}
