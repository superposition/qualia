/**
 * Unit tests for FrontierDetector
 */

import { describe, expect, it } from 'bun:test';
import { FrontierDetector } from '../src/frontier-detector';
import { OccupancyGridMap } from '../src/occupancy-grid';

describe('FrontierDetector', () => {
  const detector = new FrontierDetector();

  describe('detect', () => {
    it('should return no frontiers for a fully unknown grid', () => {
      const grid = new OccupancyGridMap({ width: 10, height: 10, resolution: 1.0 });

      const frontiers = detector.detect(grid);
      expect(frontiers).toEqual([]);
    });

    it('should return no frontiers for a fully explored (free) grid', () => {
      const grid = new OccupancyGridMap({ width: 5, height: 5, resolution: 1.0 });

      // Mark all cells as free
      for (let y = 0; y < 5; y++) {
        for (let x = 0; x < 5; x++) {
          grid.setCell(x, y, 0);
        }
      }

      const frontiers = detector.detect(grid);
      expect(frontiers).toEqual([]);
    });

    it('should detect frontiers at the boundary of free and unknown cells', () => {
      const grid = new OccupancyGridMap({ width: 10, height: 10, resolution: 1.0 });

      // Mark a 3x3 area as free in the center
      for (let y = 3; y <= 5; y++) {
        for (let x = 3; x <= 5; x++) {
          grid.setCell(x, y, 0);
        }
      }

      const frontiers = detector.detect(grid);
      expect(frontiers.length).toBeGreaterThan(0);

      // All frontier cells should be free cells adjacent to unknown cells
      const totalCells = frontiers.reduce((sum, f) => sum + f.size, 0);
      expect(totalCells).toBeGreaterThan(0);
    });

    it('should detect multiple disconnected frontiers separately', () => {
      const grid = new OccupancyGridMap({ width: 20, height: 10, resolution: 1.0 });

      // Create two separate free regions with occupied wall between them
      // Left region
      for (let y = 4; y <= 5; y++) {
        for (let x = 1; x <= 2; x++) {
          grid.setCell(x, y, 0);
        }
      }

      // Wall separating them
      for (let y = 0; y < 10; y++) {
        grid.setCell(9, y, 100);
        grid.setCell(10, y, 100);
      }

      // Right region
      for (let y = 4; y <= 5; y++) {
        for (let x = 17; x <= 18; x++) {
          grid.setCell(x, y, 0);
        }
      }

      const frontiers = detector.detect(grid);
      expect(frontiers.length).toBeGreaterThanOrEqual(2);
    });

    it('should compute correct centroid for a frontier', () => {
      const grid = new OccupancyGridMap({ width: 10, height: 10, resolution: 1.0 });

      // Create a single row of free cells at the edge of unknown space
      // Only y=0 is free, everything above is unknown
      grid.setCell(3, 0, 0);
      grid.setCell(4, 0, 0);
      grid.setCell(5, 0, 0);

      const frontiers = detector.detect(grid);
      expect(frontiers.length).toBe(1);

      const frontier = frontiers[0]!;
      // Centroid should be the average of the frontier cells
      expect(frontier.centroid.x).toBeCloseTo(4.0, 5);
      expect(frontier.centroid.y).toBeCloseTo(0.0, 5);
    });

    it('should assign unique IDs to each frontier', () => {
      const grid = new OccupancyGridMap({ width: 20, height: 10, resolution: 1.0 });

      // Two separate free regions
      grid.setCell(1, 1, 0);

      // Wall between
      for (let y = 0; y < 10; y++) {
        grid.setCell(10, y, 100);
      }

      grid.setCell(18, 1, 0);

      const frontiers = detector.detect(grid);
      const ids = frontiers.map((f) => f.id);
      const uniqueIds = new Set(ids);
      expect(uniqueIds.size).toBe(ids.length);
    });

    it('should have correct size for each frontier', () => {
      const grid = new OccupancyGridMap({ width: 10, height: 10, resolution: 1.0 });

      // Single free cell surrounded by unknown
      grid.setCell(5, 5, 0);

      const frontiers = detector.detect(grid);
      expect(frontiers.length).toBe(1);
      expect(frontiers[0]!.size).toBe(1);
      expect(frontiers[0]!.cells.length).toBe(1);
    });

    it('should not include occupied cells as frontier cells', () => {
      const grid = new OccupancyGridMap({ width: 5, height: 5, resolution: 1.0 });

      // Occupied cell adjacent to unknown should NOT be a frontier
      grid.setCell(2, 2, 100);

      const frontiers = detector.detect(grid);
      for (const frontier of frontiers) {
        for (const cell of frontier.cells) {
          expect(grid.getCell(cell.x, cell.y)).toBe(0);
        }
      }
    });
  });

  describe('isFrontierCell', () => {
    it('should return true for a free cell adjacent to unknown', () => {
      const grid = new OccupancyGridMap({ width: 5, height: 5, resolution: 1.0 });

      grid.setCell(2, 2, 0); // free cell, neighbors are unknown

      expect(FrontierDetector.isFrontierCell(grid, 2, 2)).toBe(true);
    });

    it('should return false for an unknown cell', () => {
      const grid = new OccupancyGridMap({ width: 5, height: 5, resolution: 1.0 });

      // Cell (2,2) is unknown by default
      expect(FrontierDetector.isFrontierCell(grid, 2, 2)).toBe(false);
    });

    it('should return false for an occupied cell', () => {
      const grid = new OccupancyGridMap({ width: 5, height: 5, resolution: 1.0 });

      grid.setCell(2, 2, 100);

      expect(FrontierDetector.isFrontierCell(grid, 2, 2)).toBe(false);
    });

    it('should return false for a free cell surrounded by other free cells', () => {
      const grid = new OccupancyGridMap({ width: 5, height: 5, resolution: 1.0 });

      // Mark cell and all its 4-connected neighbors as free
      grid.setCell(2, 2, 0);
      grid.setCell(3, 2, 0);
      grid.setCell(1, 2, 0);
      grid.setCell(2, 3, 0);
      grid.setCell(2, 1, 0);

      expect(FrontierDetector.isFrontierCell(grid, 2, 2)).toBe(false);
    });

    it('should return true for a free cell at the grid edge (out-of-bounds is unknown)', () => {
      const grid = new OccupancyGridMap({ width: 5, height: 5, resolution: 1.0 });

      grid.setCell(0, 0, 0);

      // Out-of-bounds returns -1 (unknown), so this is a frontier cell
      expect(FrontierDetector.isFrontierCell(grid, 0, 0)).toBe(true);
    });
  });
});
