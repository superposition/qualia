/**
 * Safety limiter for robot velocity commands
 *
 * Applies speed clamping, deadzone filtering, acceleration limiting,
 * and emergency stop logic to protect the robot and its surroundings.
 */

import type { SafetyLimiterConfig, Velocity2D } from '@qualia/types';

export class SafetyLimiter {
  private readonly config: SafetyLimiterConfig;

  constructor(config: SafetyLimiterConfig) {
    this.config = config;
  }

  /**
   * Apply speed clamping and deadzone to a velocity command.
   * - Velocities with magnitude below the deadzone are zeroed.
   * - Linear speed is clamped to [-maxLinearSpeed, maxLinearSpeed].
   * - Angular speed is clamped to [-maxAngularSpeed, maxAngularSpeed].
   */
  limit(vel: Velocity2D): Velocity2D {
    const { maxLinearSpeed, maxAngularSpeed, deadzone } = this.config;

    let linear = vel.linear;
    let angular = vel.angular;

    // Apply deadzone
    if (Math.abs(linear) < deadzone) {
      linear = 0;
    }
    if (Math.abs(angular) < deadzone) {
      angular = 0;
    }

    // Apply clamping
    linear = Math.max(-maxLinearSpeed, Math.min(maxLinearSpeed, linear));
    angular = Math.max(-maxAngularSpeed, Math.min(maxAngularSpeed, angular));

    return { linear, angular };
  }

  /**
   * Enforce acceleration limits between consecutive velocity commands.
   * @param vel     Desired velocity
   * @param prevVel Previous velocity
   * @param dt      Time step (seconds)
   */
  rampLimit(vel: Velocity2D, prevVel: Velocity2D, dt: number): Velocity2D {
    const { maxAcceleration } = this.config;
    const maxDelta = maxAcceleration * dt;

    const linearDelta = vel.linear - prevVel.linear;
    const angularDelta = vel.angular - prevVel.angular;

    const clampedLinearDelta = Math.max(
      -maxDelta,
      Math.min(maxDelta, linearDelta),
    );
    const clampedAngularDelta = Math.max(
      -maxDelta,
      Math.min(maxDelta, angularDelta),
    );

    return {
      linear: prevVel.linear + clampedLinearDelta,
      angular: prevVel.angular + clampedAngularDelta,
    };
  }

  /**
   * Determine whether an emergency stop should be triggered.
   * Returns true if the closest obstacle is within the configured
   * emergency stop distance.
   */
  emergencyStop(closestObstacleDistance: number): boolean {
    return closestObstacleDistance <= this.config.emergencyStopDistance;
  }

  /** Return a zero velocity command */
  stop(): Velocity2D {
    return { linear: 0, angular: 0 };
  }
}
