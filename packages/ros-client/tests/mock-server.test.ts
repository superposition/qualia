/**
 * Tests for MockROSBridge server
 */

import { describe, test, expect, afterEach } from 'bun:test';
import { MockROSBridge } from '../src/mock-server.ts';
import { ROSBridgeClient } from '../src/client.ts';

const wait = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

let server: MockROSBridge | null = null;

afterEach(async () => {
  if (server) {
    await server.stop();
    server = null;
  }
});

describe('MockROSBridge', () => {
  test('starts and stops cleanly', async () => {
    const port = 19200 + Math.floor(Math.random() * 800);
    server = new MockROSBridge(port);
    await server.start();
    expect(server.clientCount).toBe(0);
    await server.stop();
    server = null;
  });

  test('tracks client connections', async () => {
    const port = 19200 + Math.floor(Math.random() * 800);
    server = new MockROSBridge(port);
    await server.start();

    const c1 = new ROSBridgeClient({ url: `ws://localhost:${port}` });
    await c1.connect();
    await wait(20);
    expect(server.clientCount).toBe(1);

    const c2 = new ROSBridgeClient({ url: `ws://localhost:${port}` });
    await c2.connect();
    await wait(20);
    expect(server.clientCount).toBe(2);

    c1.disconnect();
    await wait(50);
    expect(server.clientCount).toBe(1);

    c2.disconnect();
    await wait(50);
    expect(server.clientCount).toBe(0);
  });

  test('service handler is called and returns result', async () => {
    const port = 19200 + Math.floor(Math.random() * 800);
    server = new MockROSBridge(port);
    await server.start();

    let handlerCalled = false;
    server.onServiceCall('/test_service', (args) => {
      handlerCalled = true;
      return { received: args };
    });

    const client = new ROSBridgeClient({ url: `ws://localhost:${port}` });
    await client.connect();

    const result = await client.callService('/test_service', 'test/Test', { input: 42 });
    expect(handlerCalled).toBe(true);
    expect(result).toEqual({ received: { input: 42 } });

    client.disconnect();
  });

  test('publish sends messages to all connected clients', async () => {
    const port = 19200 + Math.floor(Math.random() * 800);
    server = new MockROSBridge(port);
    await server.start();

    const received1: unknown[] = [];
    const received2: unknown[] = [];

    const c1 = new ROSBridgeClient({ url: `ws://localhost:${port}` });
    await c1.connect();
    c1.subscribe('/broadcast', 'std_msgs/String', (msg) => {
      received1.push(msg);
    });

    const c2 = new ROSBridgeClient({ url: `ws://localhost:${port}` });
    await c2.connect();
    c2.subscribe('/broadcast', 'std_msgs/String', (msg) => {
      received2.push(msg);
    });

    await wait(30);
    server.publish('/broadcast', { data: 'hello all' });
    await wait(50);

    expect(received1).toHaveLength(1);
    expect(received1[0]).toEqual({ data: 'hello all' });
    expect(received2).toHaveLength(1);
    expect(received2[0]).toEqual({ data: 'hello all' });

    c1.disconnect();
    c2.disconnect();
  });

  test('unregistered service returns result:false', async () => {
    const port = 19200 + Math.floor(Math.random() * 800);
    server = new MockROSBridge(port);
    await server.start();

    const client = new ROSBridgeClient({ url: `ws://localhost:${port}` });
    await client.connect();

    // No handler registered for /missing - the mock returns result:false
    await expect(
      client.callService('/missing', 'test/Missing', {}),
    ).rejects.toThrow('Service call failed');

    client.disconnect();
  });
});
