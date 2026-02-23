/**
 * ROS (Robot Operating System) type definitions
 * Fully typed ROS message types for use with rosbridge
 */

// ============================================================================
// Core ROS Types
// ============================================================================

/** ROS message header with timestamp and frame */
export interface ROSHeader {
  stamp: { sec: number; nanosec: number };
  frame_id: string;
}

/** ROS 3D vector */
export interface ROSVector3 {
  x: number;
  y: number;
  z: number;
}

/** ROS quaternion orientation */
export interface ROSQuaternion {
  x: number;
  y: number;
  z: number;
  w: number;
}

/** ROS 3D point */
export interface ROSPoint {
  x: number;
  y: number;
  z: number;
}

/** ROS 3D pose (position + orientation) */
export interface ROSPose {
  position: ROSPoint;
  orientation: ROSQuaternion;
}

/** ROS pose with covariance (36-element row-major) */
export interface ROSPoseWithCovariance {
  pose: ROSPose;
  covariance: number[];
}

// ============================================================================
// Geometry Messages
// ============================================================================

/** ROS Twist message (linear + angular velocity) */
export interface ROSTwist {
  linear: ROSVector3;
  angular: ROSVector3;
}

/** ROS Twist with covariance */
export interface ROSTwistWithCovariance {
  twist: ROSTwist;
  covariance: number[];
}

/** ROS stamped pose */
export interface ROSPoseStamped {
  header: ROSHeader;
  pose: ROSPose;
}

/** ROS transform (translation + rotation) */
export interface ROSTransform {
  translation: ROSVector3;
  rotation: ROSQuaternion;
}

/** ROS stamped transform */
export interface ROSTransformStamped {
  header: ROSHeader;
  child_frame_id: string;
  transform: ROSTransform;
}

// ============================================================================
// Sensor Messages
// ============================================================================

/** ROS LaserScan message */
export interface ROSLaserScan {
  header: ROSHeader;
  angle_min: number;
  angle_max: number;
  angle_increment: number;
  time_increment: number;
  scan_time: number;
  range_min: number;
  range_max: number;
  ranges: number[];
  intensities: number[];
}

/** ROS Odometry message */
export interface ROSOdometry {
  header: ROSHeader;
  child_frame_id: string;
  pose: ROSPoseWithCovariance;
  twist: ROSTwistWithCovariance;
}

/** ROS NavSatFix (GPS) message */
export interface ROSNavSatFix {
  header: ROSHeader;
  status: { status: number; service: number };
  latitude: number;
  longitude: number;
  altitude: number;
  position_covariance: number[];
  position_covariance_type: number;
}

/** ROS JointState message */
export interface ROSJointState {
  header: ROSHeader;
  name: string[];
  position: number[];
  velocity: number[];
  effort: number[];
}

/** ROS PointCloud2 message */
export interface ROSPointCloud2 {
  header: ROSHeader;
  height: number;
  width: number;
  fields: Array<{
    name: string;
    offset: number;
    datatype: number;
    count: number;
  }>;
  is_bigendian: boolean;
  point_step: number;
  row_step: number;
  data: number[];
  is_dense: boolean;
}

/** ROS Image message */
export interface ROSImage {
  header: ROSHeader;
  height: number;
  width: number;
  encoding: string;
  is_bigendian: boolean;
  step: number;
  data: number[];
}

/** ROS IMU message */
export interface ROSImu {
  header: ROSHeader;
  orientation: ROSQuaternion;
  orientation_covariance: number[];
  angular_velocity: ROSVector3;
  angular_velocity_covariance: number[];
  linear_acceleration: ROSVector3;
  linear_acceleration_covariance: number[];
}

// ============================================================================
// ROS Topic & Service Types
// ============================================================================

/** Known ROS message type strings */
export type ROSMessageType =
  | 'geometry_msgs/Twist'
  | 'geometry_msgs/PoseStamped'
  | 'geometry_msgs/TransformStamped'
  | 'sensor_msgs/LaserScan'
  | 'sensor_msgs/Image'
  | 'sensor_msgs/Imu'
  | 'sensor_msgs/JointState'
  | 'sensor_msgs/NavSatFix'
  | 'sensor_msgs/PointCloud2'
  | 'nav_msgs/Odometry'
  | 'nav_msgs/OccupancyGrid'
  | 'std_msgs/String'
  | 'std_msgs/Bool'
  | 'std_msgs/Float64'
  | (string & {});

/** ROS topic definition */
export interface ROSTopic<T = unknown> {
  name: string;
  type: ROSMessageType;
  _phantom?: T;
}

/** ROS service call */
export interface ROSServiceCall<TReq = unknown, TRes = unknown> {
  service: string;
  type: string;
  request: TReq;
  _phantomRes?: TRes;
}

/** ROS parameter value types */
export type ROSParamValue = string | number | boolean | string[] | number[] | boolean[];
