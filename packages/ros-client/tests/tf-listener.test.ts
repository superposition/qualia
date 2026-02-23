/**
 * Tests for TFListener
 */

import { describe, test, expect, beforeEach, afterEach } from 'bun:test';
import { ROSBridgeClient } from '../src/client.ts';
import { TFListener } from '../src/tf-listener.ts';
import { MockROSBridge } from '../src/mock-server.ts';
import type { ROSTransformStamped } from '@qualia/types';

let server: MockROSBridge;
let client: ROSBridgeClient;
let listener: TFListener;
let port: number;

const wait = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

/** Helper to build a ROSTransformStamped */
function makeTF(parent: string, child: string, x = 0, y = 0, z = 0): ROSTransformStamped {
  return {
    header: {
      stamp: { sec: Math.floor(Date.now() / 1000), nanosec: 0 },
      frame_id: parent,
    },
    child_frame_id: child,
    transform: {
      translation: { x, y, z },
      rotation: { x: 0, y: 0, z: 0, w: 1 },
    },
  };
}

beforeEach(async () => {
  port = 19200 + Math.floor(Math.random() * 800);
  server = new MockROSBridge(port);
  await server.start();

  client = new ROSBridgeClient({ url: `ws://localhost:${port}` });
  await client.connect();
  listener = new TFListener(client);
});

afterEach(async () => {
  client.disconnect();
  await server.stop();
});

describe('TFListener', () => {
  test('receives transform updates via /tf', async () => {
    const received: ROSTransformStamped[] = [];
    listener.onTransform((tf) => {
      received.push(tf);
    });

    const stop = listener.listen();
    await wait(30);

    const tf = makeTF('map', 'base_link', 1.0, 2.0, 0);
    server.publish('/tf', { transforms: [tf] });
    await wait(50);

    expect(received).toHaveLength(1);
    expect(received[0]!.child_frame_id).toBe('base_link');
    expect(received[0]!.transform.translation.x).toBe(1.0);

    stop();
  });

  test('getTransform returns the latest stored transform', async () => {
    const stop = listener.listen();
    await wait(30);

    server.publish('/tf', { transforms: [makeTF('map', 'odom', 1, 0, 0)] });
    await wait(50);

    server.publish('/tf', { transforms: [makeTF('map', 'odom', 2, 0, 0)] });
    await wait(50);

    const tf = listener.getTransform('map', 'odom');
    expect(tf).not.toBeNull();
    expect(tf!.transform.translation.x).toBe(2);

    stop();
  });

  test('getTransform returns null for unknown frames', () => {
    const tf = listener.getTransform('nonexistent', 'also_nonexistent');
    expect(tf).toBeNull();
  });

  test('getTransform resolves reverse lookup', async () => {
    const stop = listener.listen();
    await wait(30);

    server.publish('/tf', { transforms: [makeTF('world', 'sensor', 5, 5, 0)] });
    await wait(50);

    // Stored as world->sensor, lookup as (sensor, world) should still work
    const tf = listener.getTransform('sensor', 'world');
    expect(tf).not.toBeNull();
    expect(tf!.header.frame_id).toBe('world');

    stop();
  });

  test('onTransform callback fires for each transform in a batch', async () => {
    const received: ROSTransformStamped[] = [];
    listener.onTransform((tf) => {
      received.push(tf);
    });

    const stop = listener.listen();
    await wait(30);

    server.publish('/tf', {
      transforms: [
        makeTF('map', 'odom', 1, 0, 0),
        makeTF('odom', 'base_link', 0, 1, 0),
      ],
    });
    await wait(50);

    expect(received).toHaveLength(2);
    expect(received[0]!.child_frame_id).toBe('odom');
    expect(received[1]!.child_frame_id).toBe('base_link');

    stop();
  });

  test('unregistering onTransform callback stops it from firing', async () => {
    const received: ROSTransformStamped[] = [];
    const unregister = listener.onTransform((tf) => {
      received.push(tf);
    });

    const stop = listener.listen();
    await wait(30);

    server.publish('/tf', { transforms: [makeTF('map', 'odom', 1, 0, 0)] });
    await wait(50);
    expect(received).toHaveLength(1);

    unregister();

    server.publish('/tf', { transforms: [makeTF('map', 'odom', 2, 0, 0)] });
    await wait(50);
    expect(received).toHaveLength(1);

    stop();
  });
});
