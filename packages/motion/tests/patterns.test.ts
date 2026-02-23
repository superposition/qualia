/**
 * Unit tests for motion pattern generators
 */

import { describe, expect, test } from 'bun:test';
import {
  spin,
  figure8,
  circle,
  square,
  spiral,
  approach,
  zigzag,
} from '../src/patterns';
import type { Velocity2D } from '@qualia/types';

/** Collect up to `max` samples from an async generator */
async function collect(
  gen: AsyncGenerator<Velocity2D>,
  max: number = 100,
): Promise<Velocity2D[]> {
  const results: Velocity2D[] = [];
  for await (const vel of gen) {
    results.push(vel);
    if (results.length >= max) break;
  }
  return results;
}

describe('Motion Patterns', () => {
  describe('spin()', () => {
    test('yields angular-only velocities', async () => {
      const samples = await collect(spin(1.5, 0.3, 10), 20);
      expect(samples.length).toBeGreaterThan(0);
      for (const vel of samples) {
        expect(vel.linear).toBe(0);
        expect(vel.angular).toBe(1.5);
      }
    });

    test('terminates after duration', async () => {
      const samples = await collect(spin(1.0, 0.2, 10), 100);
      // At 10ms interval over 200ms, expect roughly 20 samples (give or take timing)
      expect(samples.length).toBeGreaterThan(0);
      expect(samples.length).toBeLessThan(100);
    });

    test('negative speed spins in opposite direction', async () => {
      const samples = await collect(spin(-2.0, 0.2, 10), 20);
      for (const vel of samples) {
        expect(vel.angular).toBe(-2.0);
      }
    });
  });

  describe('figure8()', () => {
    test('alternates angular direction', async () => {
      const samples = await collect(figure8(1.0, 1.0, 0.15, 10), 50);
      expect(samples.length).toBeGreaterThan(2);

      // Find samples with positive and negative angular velocities
      const positives = samples.filter((v) => v.angular > 0);
      const negatives = samples.filter((v) => v.angular < 0);
      expect(positives.length).toBeGreaterThan(0);
      expect(negatives.length).toBeGreaterThan(0);
    });

    test('maintains constant linear speed', async () => {
      const samples = await collect(figure8(0.8, 1.0, 0.15, 10), 50);
      for (const vel of samples) {
        expect(vel.linear).toBe(0.8);
      }
    });

    test('terminates', async () => {
      const samples = await collect(figure8(1.0, 1.0, 0.1, 10), 200);
      expect(samples.length).toBeLessThan(200);
    });
  });

  describe('zigzag()', () => {
    test('alternates angular direction', async () => {
      const samples = await collect(zigzag(1.0, 1.0, 0.15, 10), 100);
      const positives = samples.filter((v) => v.angular > 0);
      const negatives = samples.filter((v) => v.angular < 0);
      expect(positives.length).toBeGreaterThan(0);
      expect(negatives.length).toBeGreaterThan(0);
    });

    test('terminates', async () => {
      const samples = await collect(zigzag(1.0, 1.0, 0.1, 10), 200);
      expect(samples.length).toBeLessThan(200);
    });
  });

  describe('circle()', () => {
    test('yields constant velocities', async () => {
      const samples = await collect(circle(1.0, 0.5, 0.3, 10), 50);
      expect(samples.length).toBeGreaterThan(0);
      for (const vel of samples) {
        expect(vel.linear).toBe(1.0);
        expect(vel.angular).toBe(0.5);
      }
    });

    test('terminates after duration', async () => {
      const samples = await collect(circle(1.0, 0.5, 0.2, 10), 200);
      expect(samples.length).toBeLessThan(200);
    });
  });

  describe('square()', () => {
    test('has distinct straight and turn phases', async () => {
      const samples = await collect(square(1.0, 1.0, 0.15, 10), 200);
      expect(samples.length).toBeGreaterThan(0);

      const straightSamples = samples.filter(
        (v) => v.angular === 0 && v.linear > 0,
      );
      const turnSamples = samples.filter(
        (v) => v.angular !== 0 && v.linear === 0,
      );

      expect(straightSamples.length).toBeGreaterThan(0);
      expect(turnSamples.length).toBeGreaterThan(0);
    });

    test('terminates', async () => {
      const samples = await collect(square(1.0, 2.0, 0.1, 10), 500);
      expect(samples.length).toBeLessThan(500);
    });
  });

  describe('spiral()', () => {
    test('increases speed over time', async () => {
      const samples = await collect(spiral(0.5, 0.1, 1.0, 10, 10), 20);
      expect(samples.length).toBe(10);

      for (let i = 1; i < samples.length; i++) {
        expect(samples[i]!.linear).toBeGreaterThan(samples[i - 1]!.linear);
      }
    });

    test('maintains constant angular speed', async () => {
      const samples = await collect(spiral(0.5, 0.1, 1.5, 5, 10), 10);
      for (const vel of samples) {
        expect(vel.angular).toBe(1.5);
      }
    });

    test('terminates after the specified number of steps', async () => {
      const steps = 7;
      const samples = await collect(spiral(0.5, 0.1, 1.0, steps, 10), 100);
      expect(samples.length).toBe(steps);
    });
  });

  describe('approach()', () => {
    test('decelerates near target', async () => {
      const samples = await collect(approach(1.0, 0.5, 0.3, 10), 200);
      expect(samples.length).toBeGreaterThan(2);

      // Last samples should be slower than first samples
      const firstSpeed = samples[0]!.linear;
      const lastSpeed = samples[samples.length - 1]!.linear;
      expect(lastSpeed).toBeLessThanOrEqual(firstSpeed);
    });

    test('all velocities are forward-only (zero angular)', async () => {
      const samples = await collect(approach(1.0, 0.3, 0.15, 10), 200);
      for (const vel of samples) {
        expect(vel.angular).toBe(0);
        expect(vel.linear).toBeGreaterThan(0);
      }
    });

    test('terminates', async () => {
      const samples = await collect(approach(1.0, 0.2, 0.1, 10), 500);
      expect(samples.length).toBeLessThan(500);
    });
  });
});
