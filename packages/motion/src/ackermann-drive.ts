/**
 * Ackermann steering kinematics
 *
 * Implements kinematics for a car-like robot with Ackermann steering
 * geometry (front-wheel steering, rear-wheel drive).
 */

import type { AckermannDriveConfig, Velocity2D } from '@qualia/types';

export class AckermannDrive {
  private readonly config: AckermannDriveConfig;

  constructor(config: AckermannDriveConfig) {
    this.config = config;
  }

  /**
   * Compute the steering angle required for a given turning radius.
   * Uses atan(wheelBase / radius). An infinite radius (straight line)
   * returns 0. The result is clamped to maxSteeringAngle.
   */
  steeringAngle(radius: number): number {
    if (!Number.isFinite(radius) || radius === 0) {
      // Infinite radius means driving straight; zero radius is degenerate
      return radius === 0
        ? this.config.maxSteeringAngle
        : 0;
    }
    const raw = Math.atan(this.config.wheelBase / Math.abs(radius));
    const clamped = Math.min(raw, this.config.maxSteeringAngle);
    return radius > 0 ? clamped : -clamped;
  }

  /**
   * Compute the turning radius for a given steering angle.
   * Returns Infinity when the steering angle is zero (driving straight).
   */
  turningRadius(angle: number): number {
    if (angle === 0) {
      return Infinity;
    }
    return this.config.wheelBase / Math.tan(Math.abs(angle));
  }

  /**
   * Convert a forward speed and steering angle into a Velocity2D.
   * angular = speed * tan(steeringAngle) / wheelBase
   */
  toVelocity(speed: number, steeringAngle: number): Velocity2D {
    const angular =
      (speed * Math.tan(steeringAngle)) / this.config.wheelBase;
    return { linear: speed, angular };
  }

  /**
   * Clamp a velocity to the robot's limits:
   * - linear speed clamped to [-maxSpeed, maxSpeed]
   * - angular speed clamped to what maxSteeringAngle allows at current linear speed
   */
  clamp(vel: Velocity2D): Velocity2D {
    const { maxSpeed, maxSteeringAngle, wheelBase } = this.config;
    const linear = Math.max(-maxSpeed, Math.min(maxSpeed, vel.linear));

    // Maximum angular at current linear speed
    const maxAngular =
      linear === 0
        ? 0
        : Math.abs(linear) * Math.tan(maxSteeringAngle) / wheelBase;

    const angular = Math.max(-maxAngular, Math.min(maxAngular, vel.angular));
    return { linear, angular };
  }
}
