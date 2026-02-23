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
  A2AMessage,
  ROSMessageType,
  ROSTwist,
  ROSTopic,
  CapabilityCategory,
  Capability,
  HealthCheck,
  Metric,
  Result,
} from './index';

describe('Type Definitions', () => {
  test('NANDAName type accepts valid format', () => {
    const validName: NANDAName = '@hiro:qualia';
    expect(validName).toBe('@hiro:qualia');

    const anotherName: NANDAName = '@alice:fleet';
    expect(anotherName).toBe('@alice:fleet');
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

  test('A2AMessage envelope structure', () => {
    const message: A2AMessage = {
      from: 'did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK',
      to: 'did:key:z6Mkf5rGMoatrSj1f4CyvuHBeXJELe9RPdzo2PKGNCKVtZxP',
      request: {
        jsonrpc: '2.0',
        id: 1,
        method: 'navigate.to',
        params: { x: 10, y: 20 },
      },
      signature: 'sig-abc-123',
      timestamp: Date.now(),
    };

    expect(message.from).toContain('did:key:');
    expect(message.to).toContain('did:key:');
    expect(message.request.jsonrpc).toBe('2.0');
  });

  test('ROSMessageType accepts standard and custom types', () => {
    const standard: ROSMessageType = 'geometry_msgs/Twist';
    expect(standard).toBe('geometry_msgs/Twist');

    const custom: ROSMessageType = 'my_package/CustomMsg';
    expect(custom).toBe('my_package/CustomMsg');
  });

  test('ROSTwist message structure', () => {
    const twist: ROSTwist = {
      linear: { x: 1.0, y: 0, z: 0 },
      angular: { x: 0, y: 0, z: 0.5 },
    };

    expect(twist.linear.x).toBe(1.0);
    expect(twist.angular.z).toBe(0.5);
  });

  test('ROSTopic structure', () => {
    const topic: ROSTopic = {
      name: '/cmd_vel',
      type: 'geometry_msgs/Twist',
    };

    expect(topic.name).toBe('/cmd_vel');
    expect(topic.type).toBe('geometry_msgs/Twist');
  });

  test('Capability definition', () => {
    const cap: Capability = {
      name: 'lidar-navigation',
      category: 'navigate',
      description: 'Navigate using LIDAR point cloud data',
      parameters: { maxSpeed: 1.5, obstacleThreshold: 0.3 },
    };

    expect(cap.category).toBe('navigate');
    expect(cap.parameters).toBeDefined();
  });

  test('HealthCheck structure', () => {
    const health: HealthCheck = {
      status: 'healthy',
      timestamp: Date.now(),
      checks: {
        ros: true,
        network: true,
        hardware: true,
      },
    };

    expect(health.status).toBe('healthy');
    expect(health.checks.ros).toBe(true);
  });

  test('Metric data point', () => {
    const metric: Metric = {
      name: 'cpu_usage',
      value: 45.2,
      timestamp: Date.now(),
      tags: { host: 'jetson-01', role: 'ugv' },
    };

    expect(metric.name).toBe('cpu_usage');
    expect(typeof metric.value).toBe('number');
    expect(metric.tags?.host).toBe('jetson-01');
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
