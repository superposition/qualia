/**
 * Tests for ROSBridgeClient
 */

import { describe, test, expect, beforeEach, afterEach } from 'bun:test';
import { ROSBridgeClient } from '../src/client.ts';
import { MockROSBridge } from '../src/mock-server.ts';

let server: MockROSBridge;
let client: ROSBridgeClient;
let port: number;

/** Utility: wait for a short duration */
const wait = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

beforeEach(async () => {
  // Use a unique high port for each test to avoid collisions
  port = 19200 + Math.floor(Math.random() * 800);
  server = new MockROSBridge(port);
  await server.start();

  client = new ROSBridgeClient({ url: `ws://localhost:${port}` });
  await client.connect();
});

afterEach(async () => {
  client.disconnect();
  await server.stop();
});

describe('ROSBridgeClient', () => {
  // ---------------------------------------------------------------------------
  // Connection
  // ---------------------------------------------------------------------------

  test('connects to mock rosbridge', () => {
    expect(client.connected).toBe(true);
  });

  test('disconnect sets connected to false', () => {
    client.disconnect();
    expect(client.connected).toBe(false);
  });

  test('connect rejects on invalid URL', async () => {
    const badClient = new ROSBridgeClient({ url: 'ws://localhost:1' });
    await expect(badClient.connect()).rejects.toThrow();
    badClient.disconnect();
  });

  // ---------------------------------------------------------------------------
  // Subscribe / Publish
  // ---------------------------------------------------------------------------

  test('subscribe receives published messages', async () => {
    const received: unknown[] = [];
    client.subscribe<{ data: string }>('/chatter', 'std_msgs/String', (msg) => {
      received.push(msg);
    });

    await wait(20);
    server.publish('/chatter', { data: 'hello' });
    await wait(50);

    expect(received).toHaveLength(1);
    expect(received[0]).toEqual({ data: 'hello' });
  });

  test('multiple subscribers on the same topic both receive messages', async () => {
    const received1: unknown[] = [];
    const received2: unknown[] = [];

    client.subscribe<{ data: string }>('/chatter', 'std_msgs/String', (msg) => {
      received1.push(msg);
    });
    client.subscribe<{ data: string }>('/chatter', 'std_msgs/String', (msg) => {
      received2.push(msg);
    });

    await wait(20);
    server.publish('/chatter', { data: 'world' });
    await wait(50);

    expect(received1).toHaveLength(1);
    expect(received2).toHaveLength(1);
  });

  test('unsubscribe stops messages for that callback', async () => {
    const received: unknown[] = [];
    const unsub = client.subscribe<{ data: string }>('/chatter', 'std_msgs/String', (msg) => {
      received.push(msg);
    });

    await wait(20);
    server.publish('/chatter', { data: 'first' });
    await wait(50);
    expect(received).toHaveLength(1);

    unsub();
    server.publish('/chatter', { data: 'second' });
    await wait(50);
    expect(received).toHaveLength(1);
  });

  test('subscribe to different topics routes correctly', async () => {
    const topicA: unknown[] = [];
    const topicB: unknown[] = [];

    client.subscribe<{ v: number }>('/a', 'std_msgs/Float64', (msg) => {
      topicA.push(msg);
    });
    client.subscribe<{ v: number }>('/b', 'std_msgs/Float64', (msg) => {
      topicB.push(msg);
    });

    await wait(20);
    server.publish('/a', { v: 1 });
    server.publish('/b', { v: 2 });
    await wait(50);

    expect(topicA).toEqual([{ v: 1 }]);
    expect(topicB).toEqual([{ v: 2 }]);
  });

  test('onSubscribe hook fires on the mock server', async () => {
    let hookFired = false;
    server.onSubscribe('/hook_topic', () => {
      hookFired = true;
    });

    client.subscribe('/hook_topic', 'std_msgs/String', () => {});
    await wait(50);

    expect(hookFired).toBe(true);
  });

  // ---------------------------------------------------------------------------
  // Advertise / Publish
  // ---------------------------------------------------------------------------

  test('advertise and publish sends message to server', async () => {
    // The mock server ignores publish ops, so we just verify no errors
    const handle = client.advertise<{ data: string }>('/my_topic', 'std_msgs/String');
    expect(handle.publish).toBeInstanceOf(Function);
    expect(handle.unadvertise).toBeInstanceOf(Function);

    handle.publish({ data: 'hello from client' });
    handle.unadvertise();
  });

  // ---------------------------------------------------------------------------
  // Service calls
  // ---------------------------------------------------------------------------

  test('service call returns response', async () => {
    server.onServiceCall('/add_two_ints', (args) => {
      const a = args as { a: number; b: number };
      return { sum: a.a + a.b };
    });

    const result = await client.callService<{ a: number; b: number }, { sum: number }>(
      '/add_two_ints',
      'test_srvs/AddTwoInts',
      { a: 3, b: 5 },
    );

    expect(result).toEqual({ sum: 8 });
  });

  test('service call returns different responses for different args', async () => {
    server.onServiceCall('/echo', (args) => {
      return args;
    });

    const r1 = await client.callService('/echo', 'test/Echo', { msg: 'ping' });
    const r2 = await client.callService('/echo', 'test/Echo', { msg: 'pong' });

    expect(r1).toEqual({ msg: 'ping' });
    expect(r2).toEqual({ msg: 'pong' });
  });

  test('service call times out when no handler', async () => {
    await expect(
      client.callService('/nonexistent', 'test/Nope', {}, 200),
    ).rejects.toThrow('Service call failed');
  });

  test('service call rejects when not connected', async () => {
    client.disconnect();
    await expect(
      client.callService('/any', 'test/Any', {}),
    ).rejects.toThrow('Not connected');
  });

  test('service call timeout rejects with timeout error', async () => {
    // Register a handler that never responds (simulate by registering on wrong service)
    // We just do not register any handler and the mock returns result:false quickly,
    // so instead let us override the handler to delay:
    server.onServiceCall('/slow', () => {
      // The handler returns but the mock responds immediately.
      // To test a real timeout we need a handler that does NOT respond.
      // We'll abuse the mock by not registering a handler and having the mock
      // return result:false, causing a rejection (different error).
      return new Promise(() => {}); // never resolves, but mock will serialize to {}
    });

    // Since the mock serializes the return value synchronously,
    // we test the actual timeout path with a disconnected scenario.
    // Instead, create a raw server that never responds:
    const silentPort = port + 100;
    const { WebSocketServer } = await import('ws');
    const silentWss = new WebSocketServer({ port: silentPort });
    await new Promise<void>((resolve) => silentWss.on('listening', resolve));

    // eslint-disable-next-line @typescript-eslint/no-unused-vars
    silentWss.on('connection', (_ws) => {
      // Accept connection but never reply to messages
    });

    const silentClient = new ROSBridgeClient({ url: `ws://localhost:${silentPort}` });
    await silentClient.connect();

    await expect(
      silentClient.callService('/timeout_test', 'test/Timeout', {}, 200),
    ).rejects.toThrow('timed out');

    silentClient.disconnect();
    await new Promise<void>((resolve) => silentWss.close(() => resolve()));
  });

  // ---------------------------------------------------------------------------
  // Disconnect / cleanup
  // ---------------------------------------------------------------------------

  test('disconnect cleans up pending service calls', async () => {
    const silentPort = port + 200;
    const { WebSocketServer } = await import('ws');
    const silentWss = new WebSocketServer({ port: silentPort });
    await new Promise<void>((resolve) => silentWss.on('listening', resolve));
    silentWss.on('connection', () => {});

    const c2 = new ROSBridgeClient({ url: `ws://localhost:${silentPort}` });
    await c2.connect();

    const promise = c2.callService('/pending', 'test/Pending', {});
    c2.disconnect();

    await expect(promise).rejects.toThrow('disconnected');
    await new Promise<void>((resolve) => silentWss.close(() => resolve()));
  });

  // ---------------------------------------------------------------------------
  // Auto-reconnect
  // ---------------------------------------------------------------------------

  test('auto-reconnect reconnects after server restart', async () => {
    const reconnectPort = port + 300;
    const server2 = new MockROSBridge(reconnectPort);
    await server2.start();

    const reconnectClient = new ROSBridgeClient({
      url: `ws://localhost:${reconnectPort}`,
      autoReconnect: true,
      reconnectDelayMs: 100,
      maxReconnectAttempts: 5,
    });
    await reconnectClient.connect();
    expect(reconnectClient.connected).toBe(true);

    // Stop the server - the client should detect the disconnect
    await server2.stop();
    await wait(50);
    expect(reconnectClient.connected).toBe(false);

    // Restart the server on the same port
    const server3 = new MockROSBridge(reconnectPort);
    await server3.start();

    // Wait for reconnection (exponential backoff: 100ms first attempt)
    await wait(400);
    expect(reconnectClient.connected).toBe(true);

    reconnectClient.disconnect();
    await server3.stop();
  });

  test('no auto-reconnect when disabled', async () => {
    const noReconnectPort = port + 400;
    const s = new MockROSBridge(noReconnectPort);
    await s.start();

    const c = new ROSBridgeClient({
      url: `ws://localhost:${noReconnectPort}`,
      autoReconnect: false,
    });
    await c.connect();
    expect(c.connected).toBe(true);

    await s.stop();
    await wait(100);
    expect(c.connected).toBe(false);

    // Restart the server - client should NOT reconnect
    const s2 = new MockROSBridge(noReconnectPort);
    await s2.start();
    await wait(500);
    expect(c.connected).toBe(false);

    c.disconnect();
    await s2.stop();
  });
});
