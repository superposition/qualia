/**
 * Tests for A2A notifications, broadcast, and connection events
 */

import WebSocket from 'ws';
import {
  A2AServer,
  generateMockIdentity,
} from '../src';

describe('A2A Notifications & Events', () => {
  let server: A2AServer;
  let identity: { did: string; privateKey: Uint8Array };
  const PORT = 9040;

  beforeEach(() => {
    identity = generateMockIdentity();
    server = new A2AServer({
      port: PORT,
      did: identity.did,
      privateKey: identity.privateKey,
    });
    server.on('echo', async (params) => params);
  });

  afterEach(async () => {
    await server.stop();
  });

  function authenticate(ws: WebSocket, did: string): Promise<void> {
    return new Promise((resolve) => {
      ws.send(JSON.stringify({
        jsonrpc: '2.0',
        method: 'echo',
        params: {},
        auth: { from: did, signature: 'mock-sig-test' },
        id: 'auth',
      }));
      ws.once('message', () => resolve());
    });
  }

  it('should emit client:connected event', async () => {
    const connected: string[] = [];
    server.onEvent('client:connected', (did) => connected.push(did));

    await server.start();
    const ws = new WebSocket(`ws://localhost:${PORT}`);
    await new Promise((resolve) => ws.on('open', resolve));

    await authenticate(ws, 'did:key:z6MkClient1');

    expect(connected).toContain('did:key:z6MkClient1');
    ws.close();
  });

  it('should emit client:disconnected event', async () => {
    const disconnected: string[] = [];
    server.onEvent('client:disconnected', (did) => disconnected.push(did));

    await server.start();
    const ws = new WebSocket(`ws://localhost:${PORT}`);
    await new Promise((resolve) => ws.on('open', resolve));

    await authenticate(ws, 'did:key:z6MkClient2');
    ws.close();

    await new Promise((r) => setTimeout(r, 100));
    expect(disconnected).toContain('did:key:z6MkClient2');
  });

  it('should unsubscribe from events', async () => {
    const events: string[] = [];
    const unsub = server.onEvent('client:connected', (did) => events.push(did));
    unsub(); // Immediately unsubscribe

    await server.start();
    const ws = new WebSocket(`ws://localhost:${PORT}`);
    await new Promise((resolve) => ws.on('open', resolve));

    await authenticate(ws, 'did:key:z6MkClient3');

    expect(events.length).toBe(0);
    ws.close();
  });

  it('should notify specific client', async () => {
    await server.start();

    const ws = new WebSocket(`ws://localhost:${PORT}`);
    await new Promise((resolve) => ws.on('open', resolve));

    // Set up message collection before authenticate
    const messages: any[] = [];
    ws.on('message', (data) => messages.push(JSON.parse(data.toString())));

    // Authenticate (response will be captured in messages too)
    ws.send(JSON.stringify({
      jsonrpc: '2.0',
      method: 'echo',
      params: {},
      auth: { from: 'did:key:z6MkNotifyTarget', signature: 'mock-sig-test' },
      id: 'auth',
    }));
    await new Promise((r) => setTimeout(r, 200));

    // Now notify
    const sent = server.notify('did:key:z6MkNotifyTarget', 'alert', { msg: 'hello' });
    expect(sent).toBe(true);

    await new Promise((r) => setTimeout(r, 200));
    const notification = messages.find((m) => m.method === 'alert');
    expect(notification).toBeDefined();
    expect(notification.params.msg).toBe('hello');
    ws.close();
  });

  it('should return false when notifying unknown client', async () => {
    await server.start();
    const sent = server.notify('did:key:z6MkNobody', 'alert', {});
    expect(sent).toBe(false);
  });

  it('should broadcast to all connected clients', async () => {
    await server.start();

    const ws1 = new WebSocket(`ws://localhost:${PORT}`);
    const ws2 = new WebSocket(`ws://localhost:${PORT}`);
    await Promise.all([
      new Promise((r) => ws1.on('open', r)),
      new Promise((r) => ws2.on('open', r)),
    ]);

    // Set up collection before auth
    const msgs1: any[] = [];
    const msgs2: any[] = [];
    ws1.on('message', (d) => msgs1.push(JSON.parse(d.toString())));
    ws2.on('message', (d) => msgs2.push(JSON.parse(d.toString())));

    // Authenticate both
    ws1.send(JSON.stringify({
      jsonrpc: '2.0', method: 'echo', params: {},
      auth: { from: 'did:key:z6MkBroadcast1', signature: 'mock-sig-test' }, id: 'auth1',
    }));
    ws2.send(JSON.stringify({
      jsonrpc: '2.0', method: 'echo', params: {},
      auth: { from: 'did:key:z6MkBroadcast2', signature: 'mock-sig-test' }, id: 'auth2',
    }));
    await new Promise((r) => setTimeout(r, 200));

    const count = server.broadcast('fleet.update', { status: 'active' });
    expect(count).toBe(2);

    await new Promise((r) => setTimeout(r, 200));
    expect(msgs1.some((m) => m.method === 'fleet.update')).toBe(true);
    expect(msgs2.some((m) => m.method === 'fleet.update')).toBe(true);

    ws1.close();
    ws2.close();
  });
});
