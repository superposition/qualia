/**
 * Unit tests for ExplorationScorer
 */

import { describe, expect, it } from 'bun:test';
import { ExplorationScorer } from '../src/exploration-scorer';
import type { Frontier } from '@qualia/types';

function makeFrontier(id: string, centroidX: number, centroidY: number, size: number): Frontier {
  const cells = Array.from({ length: size }, (_, i) => ({
    x: centroidX + i,
    y: centroidY,
  }));
  return {
    id,
    cells,
    centroid: { x: centroidX, y: centroidY },
    size,
  };
}

describe('ExplorationScorer', () => {
  const scorer = new ExplorationScorer();
  const resolution = 1.0;

  describe('score', () => {
    it('should return empty array for empty frontier list', () => {
      const scores = scorer.score([], 0, 0, resolution);
      expect(scores).toEqual([]);
    });

    it('should score closer frontiers higher than distant ones (same size)', () => {
      const close = makeFrontier('close', 2, 0, 5);
      const far = makeFrontier('far', 20, 0, 5);

      const scores = scorer.score([close, far], 0, 0, resolution);

      const closeScore = scores.find((s) => s.frontier.id === 'close')!;
      const farScore = scores.find((s) => s.frontier.id === 'far')!;

      expect(closeScore.score).toBeGreaterThan(farScore.score);
    });

    it('should score larger frontiers higher than smaller ones (same distance)', () => {
      const large = makeFrontier('large', 10, 0, 20);
      const small = makeFrontier('small', 10, 0, 2);

      const scores = scorer.score([large, small], 0, 0, resolution);

      const largeScore = scores.find((s) => s.frontier.id === 'large')!;
      const smallScore = scores.find((s) => s.frontier.id === 'small')!;

      expect(largeScore.score).toBeGreaterThan(smallScore.score);
    });

    it('should return results sorted by score descending', () => {
      const a = makeFrontier('a', 1, 0, 10);  // close, large -> high score
      const b = makeFrontier('b', 50, 0, 2);  // far, small -> low score
      const c = makeFrontier('c', 5, 0, 8);   // medium

      const scores = scorer.score([b, c, a], 0, 0, resolution);

      for (let i = 1; i < scores.length; i++) {
        expect(scores[i - 1]!.score).toBeGreaterThanOrEqual(scores[i]!.score);
      }
    });

    it('should compute correct distance in world coordinates', () => {
      const frontier = makeFrontier('f', 3, 4, 1);

      const scores = scorer.score([frontier], 0, 0, 2.0);

      // Distance in world coords: sqrt((3*2)^2 + (4*2)^2) = sqrt(36+64) = 10
      expect(scores[0]!.distance).toBeCloseTo(10.0, 5);
    });

    it('should use informationGain equal to frontier size', () => {
      const frontier = makeFrontier('f', 5, 5, 42);

      const scores = scorer.score([frontier], 0, 0, resolution);

      expect(scores[0]!.informationGain).toBe(42);
    });

    it('should compute score as informationGain / (distance + 1)', () => {
      const frontier = makeFrontier('f', 10, 0, 20);

      const scores = scorer.score([frontier], 0, 0, 1.0);

      // distance = sqrt((10-0)^2 + 0) * 1.0 = 10
      // score = 20 / (10 + 1) = 20 / 11
      const expected = 20 / (10 + 1);
      expect(scores[0]!.score).toBeCloseTo(expected, 5);
    });

    it('should handle frontier at robot position (distance ~ 0)', () => {
      const frontier = makeFrontier('f', 0, 0, 5);

      const scores = scorer.score([frontier], 0, 0, resolution);

      // distance = 0, score = 5 / (0 + 1) = 5
      expect(scores[0]!.distance).toBeCloseTo(0.0, 5);
      expect(scores[0]!.score).toBeCloseTo(5.0, 5);
    });
  });
});
