/**
 * Differential drive kinematics
 *
 * Implements forward and inverse kinematics for a two-wheeled
 * differential drive robot.
 */

import type { DifferentialDriveConfig, Velocity2D } from '@qualia/types';

export class DifferentialDrive {
  private readonly config: DifferentialDriveConfig;

  constructor(config: DifferentialDriveConfig) {
    this.config = config;
  }

  /** Drive straight at the given speed (m/s) */
  forward(speed: number): Velocity2D {
    return { linear: speed, angular: 0 };
  }

  /** Spin in place at the given angular speed (rad/s) */
  rotate(angularSpeed: number): Velocity2D {
    return { linear: 0, angular: angularSpeed };
  }

  /**
   * Inverse kinematics: convert a 2D velocity command into
   * individual wheel speeds (rad/s).
   */
  toWheelSpeeds(vel: Velocity2D): { left: number; right: number } {
    const { wheelBase, wheelRadius } = this.config;
    // v_left  = (linear - angular * wheelBase / 2) / wheelRadius
    // v_right = (linear + angular * wheelBase / 2) / wheelRadius
    const left = (vel.linear - (vel.angular * wheelBase) / 2) / wheelRadius;
    const right = (vel.linear + (vel.angular * wheelBase) / 2) / wheelRadius;
    return { left, right };
  }

  /**
   * Forward kinematics: convert left/right wheel speeds (rad/s)
   * into a 2D velocity.
   */
  fromWheelSpeeds(left: number, right: number): Velocity2D {
    const { wheelBase, wheelRadius } = this.config;
    const linear = (wheelRadius * (left + right)) / 2;
    const angular = (wheelRadius * (right - left)) / wheelBase;
    return { linear, angular };
  }

  /**
   * Clamp velocity so that neither wheel exceeds maxWheelSpeed.
   * If a wheel would exceed the limit, both wheels are scaled
   * proportionally.
   */
  clamp(vel: Velocity2D): Velocity2D {
    const { maxWheelSpeed } = this.config;
    const wheels = this.toWheelSpeeds(vel);
    const maxAbs = Math.max(Math.abs(wheels.left), Math.abs(wheels.right));

    if (maxAbs <= maxWheelSpeed) {
      return vel;
    }

    const scale = maxWheelSpeed / maxAbs;
    return this.fromWheelSpeeds(wheels.left * scale, wheels.right * scale);
  }
}
