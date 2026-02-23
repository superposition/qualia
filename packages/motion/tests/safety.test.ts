/**
 * Unit tests for SafetyLimiter
 */

import { describe, expect, test } from 'bun:test';
import { SafetyLimiter } from '../src/safety';
import type { SafetyLimiterConfig } from '@qualia/types';

const config: SafetyLimiterConfig = {
  maxLinearSpeed: 2.0,
  maxAngularSpeed: 3.0,
  maxAcceleration: 1.0,
  deadzone: 0.05,
  emergencyStopDistance: 0.3,
};

describe('SafetyLimiter', () => {
  const limiter = new SafetyLimiter(config);

  describe('limit()', () => {
    test('velocity within limits is unchanged', () => {
      const vel = limiter.limit({ linear: 1.0, angular: 1.0 });
      expect(vel.linear).toBe(1.0);
      expect(vel.angular).toBe(1.0);
    });

    test('linear speed above max is clamped', () => {
      const vel = limiter.limit({ linear: 5.0, angular: 0 });
      expect(vel.linear).toBe(config.maxLinearSpeed);
    });

    test('angular speed above max is clamped', () => {
      const vel = limiter.limit({ linear: 0, angular: 10.0 });
      expect(vel.angular).toBe(config.maxAngularSpeed);
    });

    test('negative linear speed below -max is clamped', () => {
      const vel = limiter.limit({ linear: -5.0, angular: 0 });
      expect(vel.linear).toBe(-config.maxLinearSpeed);
    });

    test('negative angular speed below -max is clamped', () => {
      const vel = limiter.limit({ linear: 0, angular: -10.0 });
      expect(vel.angular).toBe(-config.maxAngularSpeed);
    });

    test('speeds in deadzone are zeroed', () => {
      const vel = limiter.limit({ linear: 0.01, angular: 0.03 });
      expect(vel.linear).toBe(0);
      expect(vel.angular).toBe(0);
    });

    test('speeds just above deadzone pass through', () => {
      const vel = limiter.limit({ linear: 0.06, angular: 0.06 });
      expect(vel.linear).toBe(0.06);
      expect(vel.angular).toBe(0.06);
    });

    test('negative speeds in deadzone are zeroed', () => {
      const vel = limiter.limit({ linear: -0.02, angular: -0.04 });
      expect(vel.linear).toBe(0);
      expect(vel.angular).toBe(0);
    });
  });

  describe('rampLimit()', () => {
    test('small velocity change within acceleration limit passes through', () => {
      const prev = { linear: 1.0, angular: 0.5 };
      const desired = { linear: 1.1, angular: 0.6 };
      const result = limiter.rampLimit(desired, prev, 0.5);
      expect(result.linear).toBeCloseTo(1.1, 5);
      expect(result.angular).toBeCloseTo(0.6, 5);
    });

    test('large velocity change is limited by acceleration', () => {
      const prev = { linear: 0, angular: 0 };
      const desired = { linear: 5.0, angular: 5.0 };
      const dt = 0.1;
      const result = limiter.rampLimit(desired, prev, dt);
      const maxDelta = config.maxAcceleration * dt;
      expect(result.linear).toBeCloseTo(maxDelta, 5);
      expect(result.angular).toBeCloseTo(maxDelta, 5);
    });

    test('negative acceleration is also limited', () => {
      const prev = { linear: 2.0, angular: 2.0 };
      const desired = { linear: -2.0, angular: -2.0 };
      const dt = 0.1;
      const result = limiter.rampLimit(desired, prev, dt);
      const maxDelta = config.maxAcceleration * dt;
      expect(result.linear).toBeCloseTo(prev.linear - maxDelta, 5);
      expect(result.angular).toBeCloseTo(prev.angular - maxDelta, 5);
    });

    test('zero dt means no change allowed', () => {
      const prev = { linear: 1.0, angular: 1.0 };
      const desired = { linear: 2.0, angular: 2.0 };
      const result = limiter.rampLimit(desired, prev, 0);
      expect(result.linear).toBeCloseTo(1.0, 5);
      expect(result.angular).toBeCloseTo(1.0, 5);
    });
  });

  describe('emergencyStop()', () => {
    test('obstacle within stop distance triggers stop', () => {
      expect(limiter.emergencyStop(0.1)).toBe(true);
    });

    test('obstacle at stop distance triggers stop', () => {
      expect(limiter.emergencyStop(config.emergencyStopDistance)).toBe(true);
    });

    test('obstacle beyond stop distance does not trigger', () => {
      expect(limiter.emergencyStop(1.0)).toBe(false);
    });

    test('zero distance triggers stop', () => {
      expect(limiter.emergencyStop(0)).toBe(true);
    });
  });

  describe('stop()', () => {
    test('returns zero velocity', () => {
      const vel = limiter.stop();
      expect(vel.linear).toBe(0);
      expect(vel.angular).toBe(0);
    });
  });
});
