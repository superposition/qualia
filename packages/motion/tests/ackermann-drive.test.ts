/**
 * Unit tests for AckermannDrive kinematics
 */

import { describe, expect, test } from 'bun:test';
import { AckermannDrive } from '../src/ackermann-drive';
import type { AckermannDriveConfig } from '@qualia/types';

const config: AckermannDriveConfig = {
  wheelBase: 1.0, // 1 meter between axles
  maxSteeringAngle: Math.PI / 4, // 45 degrees
  maxSpeed: 2.0, // 2 m/s
};

describe('AckermannDrive', () => {
  const drive = new AckermannDrive(config);

  describe('steeringAngle()', () => {
    test('infinite turning radius returns zero angle', () => {
      const angle = drive.steeringAngle(Infinity);
      expect(angle).toBe(0);
    });

    test('finite positive radius returns positive angle', () => {
      const angle = drive.steeringAngle(5.0);
      expect(angle).toBeGreaterThan(0);
    });

    test('negative radius returns negative angle', () => {
      const angle = drive.steeringAngle(-5.0);
      expect(angle).toBeLessThan(0);
    });

    test('small radius is clamped to maxSteeringAngle', () => {
      // Very small radius should produce an angle >= maxSteeringAngle before clamping
      const angle = drive.steeringAngle(0.01);
      expect(Math.abs(angle)).toBeCloseTo(config.maxSteeringAngle, 5);
    });

    test('zero radius returns maxSteeringAngle', () => {
      const angle = drive.steeringAngle(0);
      expect(angle).toBe(config.maxSteeringAngle);
    });
  });

  describe('turningRadius()', () => {
    test('zero angle returns Infinity', () => {
      const radius = drive.turningRadius(0);
      expect(radius).toBe(Infinity);
    });

    test('non-zero angle returns finite radius', () => {
      const radius = drive.turningRadius(0.3);
      expect(Number.isFinite(radius)).toBe(true);
      expect(radius).toBeGreaterThan(0);
    });

    test('steeringAngle and turningRadius are consistent', () => {
      const radius = 3.0;
      const angle = drive.steeringAngle(radius);
      const recoveredRadius = drive.turningRadius(angle);
      expect(recoveredRadius).toBeCloseTo(radius, 5);
    });
  });

  describe('toVelocity()', () => {
    test('zero steering angle produces zero angular velocity', () => {
      const vel = drive.toVelocity(1.0, 0);
      expect(vel.linear).toBe(1.0);
      expect(vel.angular).toBeCloseTo(0, 10);
    });

    test('positive steering angle produces angular velocity', () => {
      const vel = drive.toVelocity(1.0, 0.3);
      expect(vel.angular).toBeGreaterThan(0);
    });

    test('negative steering angle produces negative angular velocity', () => {
      const vel = drive.toVelocity(1.0, -0.3);
      expect(vel.angular).toBeLessThan(0);
    });
  });

  describe('clamp()', () => {
    test('velocity within limits is unchanged', () => {
      const vel = { linear: 1.0, angular: 0.3 };
      const clamped = drive.clamp(vel);
      expect(clamped.linear).toBe(1.0);
      expect(clamped.angular).toBe(0.3);
    });

    test('linear speed above maxSpeed is clamped', () => {
      const vel = { linear: 5.0, angular: 0 };
      const clamped = drive.clamp(vel);
      expect(clamped.linear).toBe(config.maxSpeed);
    });

    test('negative linear speed below -maxSpeed is clamped', () => {
      const vel = { linear: -5.0, angular: 0 };
      const clamped = drive.clamp(vel);
      expect(clamped.linear).toBe(-config.maxSpeed);
    });

    test('angular speed exceeding max steering angle limit is clamped', () => {
      const vel = { linear: 1.0, angular: 100.0 };
      const clamped = drive.clamp(vel);
      const maxAngular =
        Math.abs(clamped.linear) *
        Math.tan(config.maxSteeringAngle) /
        config.wheelBase;
      expect(Math.abs(clamped.angular)).toBeLessThanOrEqual(maxAngular + 1e-10);
    });
  });
});
