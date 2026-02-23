/**
 * Tests for A2A middleware system
 */

import WebSocket from 'ws';
import {
  A2AServer,
  generateMockIdentity,
  rateLimiter,
  logger,
} from '../src';
import type { Middleware, MiddlewareContext } from '../src/middleware';

describe('A2A Middleware', () => {
  let server: A2AServer;
  let identity: { did: string; privateKey: Uint8Array };
  const PORT = 9030;

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

  function sendRequest(ws: WebSocket, method: string, params: any, id: number): void {
    ws.send(JSON.stringify({
      jsonrpc: '2.0',
      method,
      params,
      auth: { from: 'did:key:z6MkTestClient', signature: 'mock-sig-test' },
      id,
    }));
  }

  it('should run middleware before handler', async () => {
    const order: string[] = [];

    const mw: Middleware = async (_ctx, next) => {
      order.push('middleware');
      return next();
    };

    server.use(mw);
    server.on('track', async () => {
      order.push('handler');
      return { ok: true };
    });

    await server.start();
    const ws = new WebSocket(`ws://localhost:${PORT}`);
    await new Promise((resolve) => ws.on('open', resolve));

    sendRequest(ws, 'track', {}, 1);
    await new Promise((resolve) => ws.on('message', resolve));

    expect(order).toEqual(['middleware', 'handler']);
    ws.close();
  });

  it('should chain multiple middlewares in order', async () => {
    const order: string[] = [];

    server.use(async (_ctx, next) => {
      order.push('first');
      return next();
    });
    server.use(async (_ctx, next) => {
      order.push('second');
      return next();
    });

    await server.start();
    const ws = new WebSocket(`ws://localhost:${PORT}`);
    await new Promise((resolve) => ws.on('open', resolve));

    sendRequest(ws, 'echo', { data: 1 }, 1);
    await new Promise((resolve) => ws.on('message', resolve));

    expect(order).toEqual(['first', 'second']);
    ws.close();
  });

  it('should allow middleware to short-circuit', async () => {
    server.use(async (ctx, _next) => {
      return {
        jsonrpc: '2.0' as const,
        error: { code: -1, message: 'Blocked by middleware' },
        id: ctx.request.id,
      };
    });

    await server.start();
    const ws = new WebSocket(`ws://localhost:${PORT}`);
    await new Promise((resolve) => ws.on('open', resolve));

    sendRequest(ws, 'echo', {}, 1);
    const response = await new Promise<string>((resolve) => {
      ws.on('message', (data) => resolve(data.toString()));
    });

    const parsed = JSON.parse(response);
    expect(parsed.error.message).toBe('Blocked by middleware');
    ws.close();
  });

  it('should provide middleware context with metadata', async () => {
    let capturedCtx: MiddlewareContext | null = null;

    server.use(async (ctx, next) => {
      ctx.metadata.set('custom', 'value');
      capturedCtx = ctx;
      return next();
    });

    await server.start();
    const ws = new WebSocket(`ws://localhost:${PORT}`);
    await new Promise((resolve) => ws.on('open', resolve));

    sendRequest(ws, 'echo', { test: true }, 1);
    await new Promise((resolve) => ws.on('message', resolve));

    expect(capturedCtx).not.toBeNull();
    expect(capturedCtx!.from).toBe('did:key:z6MkTestClient');
    expect(capturedCtx!.metadata.get('custom')).toBe('value');
    ws.close();
  });

  it('rateLimiter should block after max requests', async () => {
    server.use(rateLimiter({ maxRequests: 2, windowMs: 10000 }));

    await server.start();
    const ws = new WebSocket(`ws://localhost:${PORT}`);
    await new Promise((resolve) => ws.on('open', resolve));

    const responses: any[] = [];
    ws.on('message', (data) => {
      responses.push(JSON.parse(data.toString()));
    });

    // Send 3 requests
    for (let i = 1; i <= 3; i++) {
      sendRequest(ws, 'echo', { i }, i);
      await new Promise((r) => setTimeout(r, 50));
    }

    await new Promise((r) => setTimeout(r, 200));

    // First 2 should succeed, 3rd should be rate limited
    expect(responses.length).toBe(3);
    expect(responses[0].result).toBeDefined();
    expect(responses[1].result).toBeDefined();
    expect(responses[2].error).toBeDefined();
    expect(responses[2].error.code).toBe(-32029);

    ws.close();
  });

  it('logger middleware should not affect response', async () => {
    const logs: string[] = [];
    server.use(logger((msg) => logs.push(msg)));

    await server.start();
    const ws = new WebSocket(`ws://localhost:${PORT}`);
    await new Promise((resolve) => ws.on('open', resolve));

    sendRequest(ws, 'echo', { hello: true }, 1);
    const response = await new Promise<string>((resolve) => {
      ws.on('message', (data) => resolve(data.toString()));
    });

    const parsed = JSON.parse(response);
    expect(parsed.result).toEqual({ hello: true });
    expect(logs.length).toBe(2); // request + response log
    ws.close();
  });
});
