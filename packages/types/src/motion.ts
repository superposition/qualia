/**
 * Motion and kinematics types
 */

/** 2D velocity command (linear + angular) */
export interface Velocity2D {
  /** Forward/backward speed (m/s) */
  linear: number;
  /** Rotation speed (rad/s), positive = counter-clockwise */
  angular: number;
}

/** 2D pose (position + heading) */
export interface Pose2D {
  /** X position (meters) */
  x: number;
  /** Y position (meters) */
  y: number;
  /** Heading angle (radians) */
  theta: number;
}

/** Differential drive robot configuration */
export interface DifferentialDriveConfig {
  /** Distance between wheels (meters) */
  wheelBase: number;
  /** Wheel radius (meters) */
  wheelRadius: number;
  /** Maximum wheel speed (rad/s) */
  maxWheelSpeed: number;
}

/** Ackermann steering configuration */
export interface AckermannDriveConfig {
  /** Distance between front and rear axles (meters) */
  wheelBase: number;
  /** Maximum steering angle (radians) */
  maxSteeringAngle: number;
  /** Maximum forward speed (m/s) */
  maxSpeed: number;
}

/** Motion pattern definition */
export interface MotionPatternConfig {
  /** Pattern name */
  name: string;
  /** Velocity magnitude (m/s) */
  speed: number;
  /** Duration per cycle (seconds) */
  duration: number;
  /** Number of cycles (0 = infinite) */
  cycles: number;
}

/** Safety limiter configuration */
export interface SafetyLimiterConfig {
  /** Maximum linear speed (m/s) */
  maxLinearSpeed: number;
  /** Maximum angular speed (rad/s) */
  maxAngularSpeed: number;
  /** Maximum acceleration (m/s^2) */
  maxAcceleration: number;
  /** Deadzone threshold — velocities below this are zeroed */
  deadzone: number;
  /** Emergency stop distance (meters) */
  emergencyStopDistance: number;
}
