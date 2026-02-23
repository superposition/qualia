/**
 * Sensor reading and detection types
 */

/** Generic sensor reading */
export interface SensorReading<T = unknown> {
  /** Sensor ID */
  sensorId: string;
  /** Sensor type (e.g., 'lidar', 'camera', 'imu', 'gps') */
  type: string;
  /** Reading data */
  data: T;
  /** Unix timestamp in ms */
  timestamp: number;
  /** Reading quality [0, 1] */
  quality?: number;
}

/** Obstacle detected in sensor data */
export interface ObstacleDetection {
  /** Distance to obstacle (meters) */
  distance: number;
  /** Angle to obstacle (radians, relative to robot heading) */
  angle: number;
  /** Estimated size of obstacle (meters) */
  size?: number;
  /** Confidence [0, 1] */
  confidence: number;
}

/** Result of running a detection algorithm */
export interface DetectionResult {
  /** Whether any obstacle was detected */
  detected: boolean;
  /** All detected obstacles */
  obstacles: ObstacleDetection[];
  /** Closest obstacle distance (meters), Infinity if none */
  closestDistance: number;
  /** Timestamp of the detection */
  timestamp: number;
}

/** LIDAR scan summary for quick processing */
export interface LidarSummary {
  /** Minimum range reading (meters) */
  minRange: number;
  /** Maximum range reading (meters) */
  maxRange: number;
  /** Average range (meters) */
  avgRange: number;
  /** Number of valid readings */
  validCount: number;
  /** Total number of readings */
  totalCount: number;
  /** Sector ranges (front, left, right, back) */
  sectors: {
    front: number;
    left: number;
    right: number;
    back: number;
  };
}
