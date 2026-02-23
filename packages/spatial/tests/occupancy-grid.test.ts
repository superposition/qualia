/**
 * Unit tests for OccupancyGridMap
 */

import { describe, expect, it } from 'bun:test';
import { OccupancyGridMap } from '../src/occupancy-grid';

describe('OccupancyGridMap', () => {
  describe('constructor', () => {
    it('should create a grid with correct dimensions', () => {
      const grid = new OccupancyGridMap({ width: 10, height: 20, resolution: 0.05 });

      expect(grid.width).toBe(10);
      expect(grid.height).toBe(20);
      expect(grid.resolution).toBe(0.05);
    });

    it('should default origin to (0, 0)', () => {
      const grid = new OccupancyGridMap({ width: 5, height: 5, resolution: 1.0 });

      expect(grid.origin).toEqual({ x: 0, y: 0 });
    });

    it('should use custom origin when provided', () => {
      const grid = new OccupancyGridMap({
        width: 5,
        height: 5,
        resolution: 1.0,
        origin: { x: -2.5, y: -2.5 },
      });

      expect(grid.origin).toEqual({ x: -2.5, y: -2.5 });
    });

    it('should initialize all cells as unknown (-1)', () => {
      const grid = new OccupancyGridMap({ width: 3, height: 3, resolution: 1.0 });

      for (let y = 0; y < 3; y++) {
        for (let x = 0; x < 3; x++) {
          expect(grid.getCell(x, y)).toBe(-1);
        }
      }
    });
  });

  describe('setCell / getCell', () => {
    it('should set and get cell values', () => {
      const grid = new OccupancyGridMap({ width: 5, height: 5, resolution: 1.0 });

      grid.setCell(2, 3, 100);
      expect(grid.getCell(2, 3)).toBe(100);

      grid.setCell(0, 0, 0);
      expect(grid.getCell(0, 0)).toBe(0);
    });

    it('should return -1 for out-of-bounds coordinates', () => {
      const grid = new OccupancyGridMap({ width: 5, height: 5, resolution: 1.0 });

      expect(grid.getCell(-1, 0)).toBe(-1);
      expect(grid.getCell(0, -1)).toBe(-1);
      expect(grid.getCell(5, 0)).toBe(-1);
      expect(grid.getCell(0, 5)).toBe(-1);
      expect(grid.getCell(100, 100)).toBe(-1);
    });

    it('should silently ignore setCell on out-of-bounds coordinates', () => {
      const grid = new OccupancyGridMap({ width: 5, height: 5, resolution: 1.0 });

      // These should not throw
      grid.setCell(-1, 0, 100);
      grid.setCell(0, -1, 100);
      grid.setCell(5, 0, 100);
      grid.setCell(0, 5, 100);

      // Grid should remain unchanged (all unknown)
      expect(grid.getCell(0, 0)).toBe(-1);
    });
  });

  describe('worldToGrid', () => {
    it('should convert world coordinates to grid coordinates', () => {
      const grid = new OccupancyGridMap({ width: 100, height: 100, resolution: 0.05 });

      const cell = grid.worldToGrid(0.5, 1.0);
      expect(cell.x).toBe(10);
      expect(cell.y).toBe(20);
    });

    it('should account for grid origin offset', () => {
      const grid = new OccupancyGridMap({
        width: 100,
        height: 100,
        resolution: 0.05,
        origin: { x: -2.5, y: -2.5 },
      });

      // World (0, 0) should map to grid (50, 50) with origin at (-2.5, -2.5) and res 0.05
      const cell = grid.worldToGrid(0.0, 0.0);
      expect(cell.x).toBe(50);
      expect(cell.y).toBe(50);
    });

    it('should floor coordinates when they do not align to cell boundaries', () => {
      const grid = new OccupancyGridMap({ width: 10, height: 10, resolution: 1.0 });

      const cell = grid.worldToGrid(2.9, 3.1);
      expect(cell.x).toBe(2);
      expect(cell.y).toBe(3);
    });
  });

  describe('gridToWorld', () => {
    it('should convert grid coordinates to world coordinates (center of cell)', () => {
      const grid = new OccupancyGridMap({ width: 100, height: 100, resolution: 0.05 });

      const world = grid.gridToWorld(10, 20);
      expect(world.x).toBeCloseTo(0.525, 6);
      expect(world.y).toBeCloseTo(1.025, 6);
    });

    it('should account for grid origin offset', () => {
      const grid = new OccupancyGridMap({
        width: 100,
        height: 100,
        resolution: 1.0,
        origin: { x: -50, y: -50 },
      });

      const world = grid.gridToWorld(50, 50);
      expect(world.x).toBeCloseTo(0.5, 6);
      expect(world.y).toBeCloseTo(0.5, 6);
    });

    it('should be inverse of worldToGrid (approximately)', () => {
      const grid = new OccupancyGridMap({ width: 100, height: 100, resolution: 0.05 });

      const gx = 25;
      const gy = 30;
      const world = grid.gridToWorld(gx, gy);
      const back = grid.worldToGrid(world.x, world.y);

      expect(back.x).toBe(gx);
      expect(back.y).toBe(gy);
    });
  });

  describe('isInBounds', () => {
    it('should return true for valid coordinates', () => {
      const grid = new OccupancyGridMap({ width: 10, height: 10, resolution: 1.0 });

      expect(grid.isInBounds(0, 0)).toBe(true);
      expect(grid.isInBounds(9, 9)).toBe(true);
      expect(grid.isInBounds(5, 5)).toBe(true);
    });

    it('should return false for out-of-bounds coordinates', () => {
      const grid = new OccupancyGridMap({ width: 10, height: 10, resolution: 1.0 });

      expect(grid.isInBounds(-1, 0)).toBe(false);
      expect(grid.isInBounds(0, -1)).toBe(false);
      expect(grid.isInBounds(10, 0)).toBe(false);
      expect(grid.isInBounds(0, 10)).toBe(false);
    });
  });

  describe('updateFromLidar', () => {
    it('should mark cells along a ray as free and the endpoint as occupied', () => {
      const grid = new OccupancyGridMap({ width: 20, height: 20, resolution: 1.0 });

      // Single ray pointing right (+x direction) at range 5
      const ranges = [5.0];
      grid.updateFromLidar(
        10, 10, // robot at center
        0,      // facing right
        ranges,
        0,      // angleMin
        0,      // angleIncrement (single ray)
        10,     // rangeMax
      );

      // Cells along the ray from (10,10) to (15,10) should be free,
      // endpoint (15,10) should be occupied
      for (let x = 10; x < 15; x++) {
        expect(grid.getCell(x, 10)).toBe(0);
      }
      expect(grid.getCell(15, 10)).toBe(100);
    });

    it('should skip invalid ranges (NaN, Infinity, > rangeMax)', () => {
      const grid = new OccupancyGridMap({ width: 20, height: 20, resolution: 1.0 });

      const ranges = [NaN, Infinity, -1.0, 15.0]; // all invalid
      grid.updateFromLidar(10, 10, 0, ranges, 0, Math.PI / 2, 10);

      // Grid should remain all unknown
      for (let y = 0; y < 20; y++) {
        for (let x = 0; x < 20; x++) {
          expect(grid.getCell(x, y)).toBe(-1);
        }
      }
    });

    it('should handle multiple rays in a lidar scan', () => {
      const grid = new OccupancyGridMap({ width: 20, height: 20, resolution: 1.0 });

      // Two rays: one right, one up
      const ranges = [3.0, 4.0];
      grid.updateFromLidar(
        10, 10,
        0,
        ranges,
        0,               // angleMin = 0 (first ray along robot heading)
        Math.PI / 2,     // angleIncrement = 90 degrees
        10,
      );

      // First ray: right from (10,10), endpoint at (13,10)
      expect(grid.getCell(13, 10)).toBe(100);

      // Second ray: up from (10,10), endpoint at (10,14)
      expect(grid.getCell(10, 14)).toBe(100);
    });

    it('should handle zero-length valid ranges gracefully', () => {
      const grid = new OccupancyGridMap({ width: 10, height: 10, resolution: 1.0 });

      // Range of 0 should be skipped (<=0)
      const ranges = [0.0];
      grid.updateFromLidar(5, 5, 0, ranges, 0, 0, 10);

      // All cells should remain unknown
      for (let y = 0; y < 10; y++) {
        for (let x = 0; x < 10; x++) {
          expect(grid.getCell(x, y)).toBe(-1);
        }
      }
    });
  });

  describe('toGrid', () => {
    it('should export the grid as OccupancyGrid type', () => {
      const grid = new OccupancyGridMap({
        width: 5,
        height: 5,
        resolution: 0.1,
        origin: { x: -1, y: -1 },
      });
      grid.setCell(2, 3, 100);
      grid.setCell(1, 1, 0);

      const exported = grid.toGrid();

      expect(exported.width).toBe(5);
      expect(exported.height).toBe(5);
      expect(exported.resolution).toBe(0.1);
      expect(exported.origin).toEqual({ x: -1, y: -1 });
      expect(exported.data.length).toBe(25);
      expect(exported.data[3 * 5 + 2]).toBe(100);
      expect(exported.data[1 * 5 + 1]).toBe(0);
    });

    it('should return a copy of the data (not a reference)', () => {
      const grid = new OccupancyGridMap({ width: 3, height: 3, resolution: 1.0 });
      const exported = grid.toGrid();

      exported.data[0] = 999;
      expect(grid.getCell(0, 0)).toBe(-1);
    });
  });

  describe('toAscii', () => {
    it('should produce readable ASCII output', () => {
      const grid = new OccupancyGridMap({ width: 3, height: 3, resolution: 1.0 });

      grid.setCell(0, 0, 0);   // free
      grid.setCell(1, 1, 100); // occupied
      // (2, 2) remains unknown

      const ascii = grid.toAscii();
      const lines = ascii.split('\n');

      // y=2 row: (0,2)=unknown, (1,2)=unknown, (2,2)=unknown
      expect(lines[0]).toBe('???');
      // y=1 row: (0,1)=unknown, (1,1)=occupied, (2,1)=unknown
      expect(lines[1]).toBe('?#?');
      // y=0 row: (0,0)=free, (1,0)=unknown, (2,0)=unknown
      expect(lines[2]).toBe('.??');
    });

    it('should show robot position as R', () => {
      const grid = new OccupancyGridMap({ width: 3, height: 3, resolution: 1.0 });
      grid.setCell(1, 1, 0);

      const ascii = grid.toAscii(1, 1);
      const lines = ascii.split('\n');

      // y=1 row, x=1 should be 'R'
      expect(lines[1]![1]).toBe('R');
    });

    it('should show all cell types correctly', () => {
      const grid = new OccupancyGridMap({ width: 4, height: 1, resolution: 1.0 });

      grid.setCell(0, 0, -1);  // unknown (default)
      grid.setCell(1, 0, 0);   // free
      grid.setCell(2, 0, 100); // occupied
      grid.setCell(3, 0, 50);  // also occupied (any >0)

      const ascii = grid.toAscii();
      expect(ascii).toBe('?.##');
    });
  });
});
