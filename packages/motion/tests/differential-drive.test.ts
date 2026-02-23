/**
 * Unit tests for DifferentialDrive kinematics
 */

import { describe, expect, test } from 'bun:test';
import { DifferentialDrive } from '../src/differential-drive';
import type { DifferentialDriveConfig, Velocity2D } from '@qualia/types';

const config: DifferentialDriveConfig = {
  wheelBase: 0.5, // 50 cm between wheels
  wheelRadius: 0.1, // 10 cm wheel radius
  maxWheelSpeed: 10, // 10 rad/s max
};

describe('DifferentialDrive', () => {
  const drive = new DifferentialDrive(config);

  describe('forward()', () => {
    test('produces correct linear velocity with zero angular', () => {
      const vel = drive.forward(1.0);
      expect(vel.linear).toBe(1.0);
      expect(vel.angular).toBe(0);
    });

    test('negative speed drives backward', () => {
      const vel = drive.forward(-0.5);
      expect(vel.linear).toBe(-0.5);
      expect(vel.angular).toBe(0);
    });

    test('zero speed produces zero velocity', () => {
      const vel = drive.forward(0);
      expect(vel.linear).toBe(0);
      expect(vel.angular).toBe(0);
    });
  });

  describe('rotate()', () => {
    test('produces zero linear velocity', () => {
      const vel = drive.rotate(2.0);
      expect(vel.linear).toBe(0);
      expect(vel.angular).toBe(2.0);
    });

    test('negative angular speed spins clockwise', () => {
      const vel = drive.rotate(-1.5);
      expect(vel.linear).toBe(0);
      expect(vel.angular).toBe(-1.5);
    });
  });

  describe('toWheelSpeeds() and fromWheelSpeeds()', () => {
    test('forward motion gives equal wheel speeds', () => {
      const wheels = drive.toWheelSpeeds({ linear: 1.0, angular: 0 });
      expect(wheels.left).toBeCloseTo(wheels.right, 10);
      expect(wheels.left).toBeCloseTo(1.0 / config.wheelRadius, 10);
    });

    test('pure rotation gives opposite wheel speeds', () => {
      const wheels = drive.toWheelSpeeds({ linear: 0, angular: 2.0 });
      expect(wheels.left).toBeCloseTo(-wheels.right, 10);
    });

    test('toWheelSpeeds and fromWheelSpeeds are inverses', () => {
      const vel: Velocity2D = { linear: 0.8, angular: 1.2 };
      const wheels = drive.toWheelSpeeds(vel);
      const recovered = drive.fromWheelSpeeds(wheels.left, wheels.right);
      expect(recovered.linear).toBeCloseTo(vel.linear, 10);
      expect(recovered.angular).toBeCloseTo(vel.angular, 10);
    });

    test('fromWheelSpeeds then toWheelSpeeds round-trips', () => {
      const left = 5.0;
      const right = 8.0;
      const vel = drive.fromWheelSpeeds(left, right);
      const wheels = drive.toWheelSpeeds(vel);
      expect(wheels.left).toBeCloseTo(left, 10);
      expect(wheels.right).toBeCloseTo(right, 10);
    });

    test('zero velocity produces zero wheel speeds', () => {
      const wheels = drive.toWheelSpeeds({ linear: 0, angular: 0 });
      expect(wheels.left).toBe(0);
      expect(wheels.right).toBe(0);
    });

    test('zero wheel speeds produce zero velocity', () => {
      const vel = drive.fromWheelSpeeds(0, 0);
      expect(vel.linear).toBe(0);
      expect(vel.angular).toBe(0);
    });
  });

  describe('clamp()', () => {
    test('velocity within limits is unchanged', () => {
      const vel: Velocity2D = { linear: 0.5, angular: 0.5 };
      const clamped = drive.clamp(vel);
      expect(clamped.linear).toBeCloseTo(vel.linear, 10);
      expect(clamped.angular).toBeCloseTo(vel.angular, 10);
    });

    test('velocity exceeding max wheel speed is scaled down', () => {
      // Create a velocity that would exceed maxWheelSpeed
      const vel: Velocity2D = { linear: 2.0, angular: 5.0 };
      const clamped = drive.clamp(vel);
      const wheels = drive.toWheelSpeeds(clamped);
      expect(Math.abs(wheels.left)).toBeLessThanOrEqual(config.maxWheelSpeed + 1e-10);
      expect(Math.abs(wheels.right)).toBeLessThanOrEqual(config.maxWheelSpeed + 1e-10);
    });

    test('clamping preserves velocity direction ratio', () => {
      const vel: Velocity2D = { linear: 2.0, angular: 4.0 };
      const clamped = drive.clamp(vel);
      // The ratio of linear to angular should be preserved
      if (clamped.linear !== 0) {
        expect(clamped.angular / clamped.linear).toBeCloseTo(
          vel.angular / vel.linear,
          5,
        );
      }
    });
  });
});
