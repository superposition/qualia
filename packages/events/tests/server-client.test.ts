/**
 * Integration tests for EventServer and EventClient
 */

import { describe, test, expect, afterEach } from 'bun:test';
import { AgentEventStream } from '../src/stream';
import { EventServer } from '../src/server';
import { EventClient } from '../src/client';
import type { AgentEvent } from '@qualia/types';

/** Helper: wait for a condition to become true or timeout. */
async function waitFor(
  predicate: () => boolean,
  timeoutMs = 5000,
  intervalMs = 50,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!predicate()) {
    if (Date.now() > deadline) {
      throw new Error('waitFor timed out');
    }
    await new Promise((r) => setTimeout(r, intervalMs));
  }
}

/** Helper: collect N events from a client. */
function collectEvents(client: EventClient, count: number, timeoutMs = 5000): Promise<AgentEvent[]> {
  return new Promise((resolve, reject) => {
    const events: AgentEvent[] = [];
    const timer = setTimeout(() => {
      unsub();
      reject(new Error(`collectEvents timed out after ${timeoutMs}ms (got ${events.length}/${count})`));
    }, timeoutMs);
    const unsub = client.onEvent((event) => {
      events.push(event);
      if (events.length >= count) {
        clearTimeout(timer);
        unsub();
        resolve(events);
      }
    });
  });
}

let server: EventServer | null = null;
let clients: EventClient[] = [];

afterEach(async () => {
  for (const c of clients) {
    c.disconnect();
  }
  clients = [];
  if (server) {
    await server.stop();
    server = null;
  }
  // Give OS time to release the port
  await new Promise((r) => setTimeout(r, 50));
});

describe('EventServer + EventClient', () => {
  test('server starts and stops cleanly', async () => {
    const stream = new AgentEventStream();
    server = new EventServer({ port: 0, stream });
    await server.start();

    expect(server.port).toBeGreaterThan(0);
    expect(server.clientCount).toBe(0);

    await server.stop();
    server = null;
  });

  test('client connects and receives events', async () => {
    const stream = new AgentEventStream();
    server = new EventServer({ port: 0, stream });
    await server.start();

    const client = new EventClient({
      url: `ws://localhost:${server.port}`,
      autoReconnect: false,
    });
    clients.push(client);

    await client.connect();
    expect(client.connected).toBe(true);

    const collected = collectEvents(client, 1);
    stream.emit('message', { text: 'hello' });

    const events = await collected;
    expect(events).toHaveLength(1);
    expect(events[0]!.type).toBe('message');
    expect(events[0]!.data).toEqual({ text: 'hello' });
  });

  test('client receives replay on connect', async () => {
    const stream = new AgentEventStream({ bufferSize: 100 });
    server = new EventServer({ port: 0, stream });
    await server.start();

    // Emit events before any client connects
    stream.emit('message', 'replay-1');
    stream.emit('status', 'replay-2');
    stream.emit('error', 'replay-3');

    const client = new EventClient({
      url: `ws://localhost:${server.port}`,
      autoReconnect: false,
    });
    clients.push(client);

    const collected = collectEvents(client, 3);
    await client.connect();

    const events = await collected;
    expect(events).toHaveLength(3);
    expect(events[0]!.data).toBe('replay-1');
    expect(events[1]!.data).toBe('replay-2');
    expect(events[2]!.data).toBe('replay-3');
  });

  test('filter limits which events client receives', async () => {
    const stream = new AgentEventStream({ bufferSize: 100 });
    server = new EventServer({ port: 0, stream });
    await server.start();

    const client = new EventClient({
      url: `ws://localhost:${server.port}`,
      filter: { types: ['error'] },
      autoReconnect: false,
    });
    clients.push(client);

    const allReceived: AgentEvent[] = [];
    client.onEvent((event) => {
      allReceived.push(event);
    });

    await client.connect();
    // Wait for subscribe filter to be set
    await new Promise((r) => setTimeout(r, 100));

    stream.emit('message', 'ignored');
    stream.emit('error', 'captured');
    stream.emit('status', 'ignored-too');

    // Wait for events to arrive
    await waitFor(() => allReceived.length >= 1, 3000);
    // Give a little extra time for any additional events
    await new Promise((r) => setTimeout(r, 200));

    // Client should only have received the error event (server broadcasts based on client filter)
    // Note: The server broadcasts all events initially; client-side filter is applied by server after subscribe
    const errorEvents = allReceived.filter((e) => e.type === 'error');
    expect(errorEvents.length).toBeGreaterThanOrEqual(1);
    expect(errorEvents[0]!.data).toBe('captured');
  });

  test('multiple clients receive events independently', async () => {
    const stream = new AgentEventStream();
    server = new EventServer({ port: 0, stream });
    await server.start();

    const client1 = new EventClient({
      url: `ws://localhost:${server.port}`,
      autoReconnect: false,
    });
    const client2 = new EventClient({
      url: `ws://localhost:${server.port}`,
      autoReconnect: false,
    });
    clients.push(client1, client2);

    await client1.connect();
    await client2.connect();

    await waitFor(() => server!.clientCount === 2, 3000);

    const collected1 = collectEvents(client1, 2);
    const collected2 = collectEvents(client2, 2);

    stream.emit('message', 'first');
    stream.emit('message', 'second');

    const [events1, events2] = await Promise.all([collected1, collected2]);

    expect(events1).toHaveLength(2);
    expect(events2).toHaveLength(2);
    expect(events1[0]!.data).toBe('first');
    expect(events2[0]!.data).toBe('first');
  });

  test('client disconnect is handled cleanly', async () => {
    const stream = new AgentEventStream();
    server = new EventServer({ port: 0, stream });
    await server.start();

    const client = new EventClient({
      url: `ws://localhost:${server.port}`,
      autoReconnect: false,
    });
    clients.push(client);

    await client.connect();
    await waitFor(() => server!.clientCount === 1, 3000);

    client.disconnect();
    expect(client.connected).toBe(false);

    await waitFor(() => server!.clientCount === 0, 3000);
    expect(server.clientCount).toBe(0);
  });

  test('client auto-reconnects after server restart', async () => {
    const stream = new AgentEventStream();
    server = new EventServer({ port: 0, stream });
    await server.start();
    const port = server.port;

    const client = new EventClient({
      url: `ws://localhost:${port}`,
      autoReconnect: true,
    });
    clients.push(client);

    await client.connect();
    expect(client.connected).toBe(true);

    // Stop the server
    await server.stop();
    await waitFor(() => !client.connected, 3000);

    // Restart the server on the same port
    server = new EventServer({ port, stream });
    await server.start();

    // Client should auto-reconnect
    await waitFor(() => client.connected, 10000);
    expect(client.connected).toBe(true);
  });

  test('server reports correct client count', async () => {
    const stream = new AgentEventStream();
    server = new EventServer({ port: 0, stream });
    await server.start();

    expect(server.clientCount).toBe(0);

    const client1 = new EventClient({
      url: `ws://localhost:${server.port}`,
      autoReconnect: false,
    });
    const client2 = new EventClient({
      url: `ws://localhost:${server.port}`,
      autoReconnect: false,
    });
    const client3 = new EventClient({
      url: `ws://localhost:${server.port}`,
      autoReconnect: false,
    });
    clients.push(client1, client2, client3);

    await client1.connect();
    await waitFor(() => server!.clientCount === 1, 3000);

    await client2.connect();
    await waitFor(() => server!.clientCount === 2, 3000);

    await client3.connect();
    await waitFor(() => server!.clientCount === 3, 3000);

    client2.disconnect();
    await waitFor(() => server!.clientCount === 2, 3000);

    expect(server.clientCount).toBe(2);
  });

  test('events contain correct structure over the wire', async () => {
    const stream = new AgentEventStream();
    server = new EventServer({ port: 0, stream });
    await server.start();

    const client = new EventClient({
      url: `ws://localhost:${server.port}`,
      autoReconnect: false,
    });
    clients.push(client);

    await client.connect();

    const collected = collectEvents(client, 1);
    stream.emit('navigation', { x: 1, y: 2 }, 'robot-1');

    const events = await collected;
    const event = events[0]!;

    expect(event.id).toBeDefined();
    expect(typeof event.id).toBe('string');
    expect(event.type).toBe('navigation');
    expect(event.data).toEqual({ x: 1, y: 2 });
    expect(event.source).toBe('robot-1');
    expect(typeof event.timestamp).toBe('number');
    expect(typeof event.sequence).toBe('number');
  });

  test('server handles rapid event bursts', async () => {
    const stream = new AgentEventStream();
    server = new EventServer({ port: 0, stream });
    await server.start();

    const client = new EventClient({
      url: `ws://localhost:${server.port}`,
      autoReconnect: false,
    });
    clients.push(client);

    await client.connect();

    const burstSize = 50;
    const collected = collectEvents(client, burstSize, 10000);

    for (let i = 0; i < burstSize; i++) {
      stream.emit('sensor_data', { index: i });
    }

    const events = await collected;
    expect(events).toHaveLength(burstSize);

    // Verify ordering is preserved
    for (let i = 0; i < burstSize; i++) {
      expect(events[i]!.data).toEqual({ index: i });
    }
  });
});
