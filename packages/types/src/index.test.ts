/**
 * Tests for QUALIA Shared Type Definitions
 */

import { describe, test, expect } from 'bun:test';
import type {
  NANDAName,
  DID,
  Address,
  NANDA,
  Passport,
  A2ARequest,
  A2AResponse,
  Result,
  // ROS types
  ROSHeader,
  ROSLaserScan,
  ROSOdometry,
  ROSPoseStamped,
  ROSNavSatFix,
  ROSJointState,
  ROSPointCloud2,
  ROSTwist,
  ROSTopic,
  ROSServiceCall,
  ROSMessageType,
  ROSImu,
  // Event types
  AgentEventType,
  AgentEvent,
  EventFilter,
  // Motion types
  Velocity2D,
  Pose2D,
  DifferentialDriveConfig,
  AckermannDriveConfig,
  SafetyLimiterConfig,
  // Grid types
  OccupancyGrid,
  GridCell,
  Frontier,
  ExplorationScore,
  CellState,
  // Swarm types
  SwarmMessage,
  AgentPosition,
  AreaClaim,
  FleetAlert,
  TaskProposal,
  TaskBid,
  // Sensor types
  SensorReading,
  ObstacleDetection,
  DetectionResult,
  LidarSummary,
} from './index';

describe('Core Type Definitions', () => {
  test('NANDAName type accepts valid format', () => {
    const validName: NANDAName = '@hiro:qualia';
    expect(validName).toBe('@hiro:qualia');

    const anotherName: NANDAName = '@alice:anthropic';
    expect(anotherName).toBe('@alice:anthropic');
  });

  test('DID type accepts valid format', () => {
    const validDid: DID = 'did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK';
    expect(validDid).toContain('did:key:z');
  });

  test('Address type accepts valid Ethereum format', () => {
    const validAddress: Address = '0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0';
    expect(validAddress).toMatch(/^0x[a-fA-F0-9]{40}$/);
  });

  test('NANDA interface has required fields', () => {
    const nanda: NANDA = {
      name: '@test:agent',
      did: 'did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK',
      capabilities: ['navigate', 'perceive'],
      wallet: '0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0',
      isActive: true,
      registeredAt: Date.now(),
    };

    expect(nanda.name).toContain('@');
    expect(nanda.capabilities).toBeInstanceOf(Array);
    expect(nanda.capabilities.length).toBeGreaterThan(0);
  });

  test('Passport interface structure', () => {
    const passport: Passport = {
      did: 'did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK',
      publicKey: 'abc123',
      capabilities: ['navigate'],
      signature: 'sig123',
      issuedAt: Date.now(),
    };

    expect(passport.did).toContain('did:key:');
    expect(passport.capabilities).toBeInstanceOf(Array);
    expect(typeof passport.issuedAt).toBe('number');
  });

  test('Passport with optional expiresAt', () => {
    const passport: Passport = {
      did: 'did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK',
      publicKey: 'abc123',
      capabilities: ['navigate'],
      signature: 'sig123',
      issuedAt: 1000,
      expiresAt: 2000,
    };

    expect(passport.expiresAt).toBe(2000);
  });

  test('A2ARequest conforms to JSON-RPC 2.0', () => {
    const request: A2ARequest = {
      jsonrpc: '2.0',
      id: 1,
      method: 'test.method',
      params: { foo: 'bar' },
    };

    expect(request.jsonrpc).toBe('2.0');
    expect(request.method).toBe('test.method');
  });

  test('A2AResponse can have result or error', () => {
    const successResponse: A2AResponse = {
      jsonrpc: '2.0',
      id: 1,
      result: { success: true },
    };

    const errorResponse: A2AResponse = {
      jsonrpc: '2.0',
      id: 1,
      error: {
        code: -32600,
        message: 'Invalid Request',
      },
    };

    expect(successResponse.result).toBeDefined();
    expect(errorResponse.error).toBeDefined();
  });

  test('Result type for success case', () => {
    const success: Result<number> = {
      ok: true,
      value: 42,
    };

    expect(success.ok).toBe(true);
    if (success.ok) {
      expect(success.value).toBe(42);
    }
  });

  test('Result type for error case', () => {
    const failure: Result<number> = {
      ok: false,
      error: new Error('Something went wrong'),
    };

    expect(failure.ok).toBe(false);
    if (!failure.ok) {
      expect(failure.error).toBeInstanceOf(Error);
    }
  });
});

describe('ROS Type Definitions', () => {
  test('ROSHeader has stamp and frame_id', () => {
    const header: ROSHeader = {
      stamp: { sec: 1234567890, nanosec: 0 },
      frame_id: 'base_link',
    };
    expect(header.frame_id).toBe('base_link');
    expect(header.stamp.sec).toBe(1234567890);
  });

  test('ROSTwist has linear and angular vectors', () => {
    const twist: ROSTwist = {
      linear: { x: 1.0, y: 0, z: 0 },
      angular: { x: 0, y: 0, z: 0.5 },
    };
    expect(twist.linear.x).toBe(1.0);
    expect(twist.angular.z).toBe(0.5);
  });

  test('ROSLaserScan has ranges and angles', () => {
    const scan: ROSLaserScan = {
      header: { stamp: { sec: 0, nanosec: 0 }, frame_id: 'laser' },
      angle_min: -Math.PI,
      angle_max: Math.PI,
      angle_increment: 0.01,
      time_increment: 0,
      scan_time: 0.1,
      range_min: 0.1,
      range_max: 30.0,
      ranges: [1.0, 2.0, 3.0],
      intensities: [100, 200, 300],
    };
    expect(scan.ranges.length).toBe(3);
    expect(scan.range_max).toBe(30.0);
  });

  test('ROSOdometry has pose and twist with covariance', () => {
    const odom: ROSOdometry = {
      header: { stamp: { sec: 0, nanosec: 0 }, frame_id: 'odom' },
      child_frame_id: 'base_link',
      pose: {
        pose: {
          position: { x: 1, y: 2, z: 0 },
          orientation: { x: 0, y: 0, z: 0, w: 1 },
        },
        covariance: new Array(36).fill(0),
      },
      twist: {
        twist: { linear: { x: 0.5, y: 0, z: 0 }, angular: { x: 0, y: 0, z: 0.1 } },
        covariance: new Array(36).fill(0),
      },
    };
    expect(odom.child_frame_id).toBe('base_link');
    expect(odom.pose.pose.position.x).toBe(1);
  });

  test('ROSPoseStamped has header and pose', () => {
    const goal: ROSPoseStamped = {
      header: { stamp: { sec: 0, nanosec: 0 }, frame_id: 'map' },
      pose: {
        position: { x: 5, y: 10, z: 0 },
        orientation: { x: 0, y: 0, z: 0, w: 1 },
      },
    };
    expect(goal.pose.position.x).toBe(5);
    expect(goal.header.frame_id).toBe('map');
  });

  test('ROSNavSatFix has GPS coordinates', () => {
    const gps: ROSNavSatFix = {
      header: { stamp: { sec: 0, nanosec: 0 }, frame_id: 'gps' },
      status: { status: 0, service: 1 },
      latitude: 37.7749,
      longitude: -122.4194,
      altitude: 10.0,
      position_covariance: new Array(9).fill(0),
      position_covariance_type: 1,
    };
    expect(gps.latitude).toBe(37.7749);
    expect(gps.longitude).toBe(-122.4194);
  });

  test('ROSJointState has name, position, velocity, effort arrays', () => {
    const joints: ROSJointState = {
      header: { stamp: { sec: 0, nanosec: 0 }, frame_id: '' },
      name: ['joint1', 'joint2'],
      position: [0.5, 1.0],
      velocity: [0.1, 0.2],
      effort: [10, 20],
    };
    expect(joints.name.length).toBe(2);
    expect(joints.position[0]).toBe(0.5);
  });

  test('ROSPointCloud2 has structured fields', () => {
    const cloud: ROSPointCloud2 = {
      header: { stamp: { sec: 0, nanosec: 0 }, frame_id: 'velodyne' },
      height: 1,
      width: 100,
      fields: [{ name: 'x', offset: 0, datatype: 7, count: 1 }],
      is_bigendian: false,
      point_step: 16,
      row_step: 1600,
      data: [],
      is_dense: true,
    };
    expect(cloud.width).toBe(100);
    expect(cloud.fields[0]?.name).toBe('x');
  });

  test('ROSImu has orientation and velocity data', () => {
    const imu: ROSImu = {
      header: { stamp: { sec: 0, nanosec: 0 }, frame_id: 'imu_link' },
      orientation: { x: 0, y: 0, z: 0, w: 1 },
      orientation_covariance: new Array(9).fill(0),
      angular_velocity: { x: 0, y: 0, z: 0.01 },
      angular_velocity_covariance: new Array(9).fill(0),
      linear_acceleration: { x: 0, y: 0, z: 9.81 },
      linear_acceleration_covariance: new Array(9).fill(0),
    };
    expect(imu.linear_acceleration.z).toBe(9.81);
  });

  test('ROSTopic has generic type parameter', () => {
    const topic: ROSTopic<ROSTwist> = {
      name: '/cmd_vel',
      type: 'geometry_msgs/Twist',
    };
    expect(topic.name).toBe('/cmd_vel');
    expect(topic.type).toBe('geometry_msgs/Twist');
  });

  test('ROSServiceCall has request/response generics', () => {
    interface SetBoolReq { data: boolean }
    interface SetBoolRes { success: boolean; message: string }
    const call: ROSServiceCall<SetBoolReq, SetBoolRes> = {
      service: '/enable_motor',
      type: 'std_srvs/SetBool',
      request: { data: true },
    };
    expect(call.request.data).toBe(true);
  });

  test('ROSMessageType accepts standard and custom types', () => {
    const standard: ROSMessageType = 'geometry_msgs/Twist';
    const custom: ROSMessageType = 'my_package/MyMsg';
    expect(standard).toBe('geometry_msgs/Twist');
    expect(custom).toBe('my_package/MyMsg');
  });
});

describe('Event Type Definitions', () => {
  test('AgentEvent has required fields with generic data', () => {
    const event: AgentEvent<{ tool: string; args: string[] }> = {
      id: 'evt-1',
      type: 'tool_call',
      data: { tool: 'navigate', args: ['forward'] },
      timestamp: Date.now(),
      sequence: 1,
      source: 'did:key:z6MkTest',
    };
    expect(event.type).toBe('tool_call');
    expect(event.data.tool).toBe('navigate');
    expect(event.sequence).toBe(1);
  });

  test('AgentEventType union covers all event kinds', () => {
    const types: AgentEventType[] = [
      'tool_call', 'tool_result', 'message', 'status',
      'sensor_data', 'error', 'navigation', 'discovery',
    ];
    expect(types.length).toBe(8);
  });

  test('EventFilter supports type and source filtering', () => {
    const filter: EventFilter = {
      types: ['tool_call', 'error'],
      sources: ['did:key:z6MkAgent1'],
      afterSequence: 100,
    };
    expect(filter.types?.length).toBe(2);
    expect(filter.afterSequence).toBe(100);
  });

  test('EventFilter can be empty for all events', () => {
    const filter: EventFilter = {};
    expect(filter.types).toBeUndefined();
  });
});

describe('Motion Type Definitions', () => {
  test('Velocity2D has linear and angular', () => {
    const vel: Velocity2D = { linear: 1.0, angular: 0.5 };
    expect(vel.linear).toBe(1.0);
    expect(vel.angular).toBe(0.5);
  });

  test('Pose2D has x, y, theta', () => {
    const pose: Pose2D = { x: 1.0, y: 2.0, theta: Math.PI / 4 };
    expect(pose.theta).toBeCloseTo(Math.PI / 4);
  });

  test('DifferentialDriveConfig has geometry params', () => {
    const config: DifferentialDriveConfig = {
      wheelBase: 0.3,
      wheelRadius: 0.05,
      maxWheelSpeed: 10.0,
    };
    expect(config.wheelBase).toBe(0.3);
  });

  test('AckermannDriveConfig has steering params', () => {
    const config: AckermannDriveConfig = {
      wheelBase: 2.5,
      maxSteeringAngle: Math.PI / 6,
      maxSpeed: 15.0,
    };
    expect(config.maxSteeringAngle).toBeCloseTo(Math.PI / 6);
  });

  test('SafetyLimiterConfig has all safety params', () => {
    const config: SafetyLimiterConfig = {
      maxLinearSpeed: 1.0,
      maxAngularSpeed: 2.0,
      maxAcceleration: 0.5,
      deadzone: 0.01,
      emergencyStopDistance: 0.3,
    };
    expect(config.emergencyStopDistance).toBe(0.3);
    expect(config.deadzone).toBe(0.01);
  });
});

describe('Grid Type Definitions', () => {
  test('OccupancyGrid has dimensions and data', () => {
    const grid: OccupancyGrid = {
      width: 100,
      height: 100,
      resolution: 0.05,
      origin: { x: -2.5, y: -2.5 },
      data: new Array(10000).fill(-1),
    };
    expect(grid.data.length).toBe(grid.width * grid.height);
    expect(grid.resolution).toBe(0.05);
  });

  test('GridCell has state and probability', () => {
    const cell: GridCell = {
      x: 10,
      y: 20,
      state: 'free',
      probability: 0.1,
      visitCount: 5,
    };
    expect(cell.state).toBe('free');
    expect(cell.probability).toBeLessThan(0.5);
  });

  test('CellState covers all states', () => {
    const states: CellState[] = ['free', 'occupied', 'unknown'];
    expect(states.length).toBe(3);
  });

  test('Frontier has cells and centroid', () => {
    const frontier: Frontier = {
      id: 'f-1',
      cells: [{ x: 10, y: 20 }, { x: 11, y: 20 }, { x: 12, y: 20 }],
      centroid: { x: 11, y: 20 },
      size: 3,
    };
    expect(frontier.size).toBe(frontier.cells.length);
  });

  test('ExplorationScore combines distance and info gain', () => {
    const score: ExplorationScore = {
      frontier: {
        id: 'f-1',
        cells: [{ x: 10, y: 20 }],
        centroid: { x: 10, y: 20 },
        size: 1,
      },
      distance: 5.0,
      informationGain: 0.8,
      score: 0.8 / 5.0,
    };
    expect(score.score).toBeCloseTo(0.16);
  });
});

describe('Swarm Type Definitions', () => {
  test('SwarmMessage has generic payload', () => {
    const msg: SwarmMessage<AgentPosition> = {
      type: 'position_update',
      from: 'did:key:z6MkTest',
      data: { did: 'did:key:z6MkTest', x: 1, y: 2, theta: 0, timestamp: Date.now() },
      timestamp: Date.now(),
    };
    expect(msg.type).toBe('position_update');
    expect(msg.data.x).toBe(1);
  });

  test('AgentPosition has coordinates and heading', () => {
    const pos: AgentPosition = {
      did: 'did:key:z6MkAgent1',
      x: 5.0,
      y: 10.0,
      theta: Math.PI / 2,
      timestamp: Date.now(),
    };
    expect(pos.theta).toBeCloseTo(Math.PI / 2);
  });

  test('AreaClaim has center, radius, and expiry', () => {
    const claim: AreaClaim = {
      id: 'claim-1',
      agentDid: 'did:key:z6MkAgent1',
      center: { x: 5, y: 5 },
      radius: 2.0,
      expiresAt: Date.now() + 60000,
    };
    expect(claim.radius).toBe(2.0);
    expect(claim.expiresAt).toBeGreaterThan(Date.now());
  });

  test('FleetAlert has severity and location', () => {
    const alert: FleetAlert = {
      type: 'obstacle',
      location: { x: 3, y: 4 },
      severity: 3,
      message: 'Large obstacle detected ahead',
    };
    expect(alert.severity).toBeGreaterThanOrEqual(1);
    expect(alert.severity).toBeLessThanOrEqual(5);
  });

  test('TaskProposal and TaskBid work together', () => {
    const proposal: TaskProposal = {
      id: 'task-1',
      description: 'Explore sector 7',
      requiredCapabilities: ['navigate', 'perceive'],
      location: { x: 20, y: 30 },
    };
    const bid: TaskBid = {
      taskId: proposal.id,
      agentDid: 'did:key:z6MkBidder',
      cost: 50,
      estimatedTime: 120000,
    };
    expect(bid.taskId).toBe(proposal.id);
    expect(bid.cost).toBe(50);
  });
});

describe('Sensor Type Definitions', () => {
  test('SensorReading has generic data', () => {
    const reading: SensorReading<number[]> = {
      sensorId: 'lidar-1',
      type: 'lidar',
      data: [1.0, 2.0, 3.0],
      timestamp: Date.now(),
      quality: 0.95,
    };
    expect(reading.data.length).toBe(3);
    expect(reading.quality).toBe(0.95);
  });

  test('ObstacleDetection has distance, angle, confidence', () => {
    const obstacle: ObstacleDetection = {
      distance: 1.5,
      angle: 0.3,
      size: 0.5,
      confidence: 0.9,
    };
    expect(obstacle.distance).toBe(1.5);
    expect(obstacle.confidence).toBeLessThanOrEqual(1);
  });

  test('DetectionResult aggregates obstacles', () => {
    const result: DetectionResult = {
      detected: true,
      obstacles: [
        { distance: 1.5, angle: 0.3, confidence: 0.9 },
        { distance: 3.0, angle: -0.5, confidence: 0.7 },
      ],
      closestDistance: 1.5,
      timestamp: Date.now(),
    };
    expect(result.detected).toBe(true);
    expect(result.obstacles.length).toBe(2);
    expect(result.closestDistance).toBe(1.5);
  });

  test('DetectionResult with no obstacles', () => {
    const result: DetectionResult = {
      detected: false,
      obstacles: [],
      closestDistance: Infinity,
      timestamp: Date.now(),
    };
    expect(result.detected).toBe(false);
    expect(result.closestDistance).toBe(Infinity);
  });

  test('LidarSummary has sector data', () => {
    const summary: LidarSummary = {
      minRange: 0.5,
      maxRange: 10.0,
      avgRange: 5.0,
      validCount: 360,
      totalCount: 360,
      sectors: { front: 3.0, left: 5.0, right: 4.0, back: 8.0 },
    };
    expect(summary.sectors.front).toBe(3.0);
    expect(summary.validCount).toBe(summary.totalCount);
  });
});
