import { describe, test, expect, beforeAll, afterAll, beforeEach } from 'bun:test';
import {
  A2AClient,
  A2AServer,
  JsonRpcErrorCode,
  registerAgent,
  discover,
  getAgentMetadata,
  searchAgents,
  clearRegistry,
  unregisterAgent,
} from './index';
import { generateDID, generateKeypair, publicKeyToDID } from '@qualia/passport';

// Use dynamic ports to avoid conflicts
let serverPort = 0;

function getPort(): number {
  return 19000 + Math.floor(Math.random() * 1000);
}

describe('Discovery', () => {
  beforeEach(() => {
    clearRegistry();
  });

  test('registerAgent and discover by capability', async () => {
    const { did } = generateDID();

    await registerAgent({
      did,
      name: 'nav-robot',
      capabilities: [{ name: 'navigate', version: '1.0' }],
      endpoints: { a2a: 'ws://localhost:9999' },
    });

    const results = await discover('navigate');
    expect(results).toContain(did);

    const empty = await discover('fly');
    expect(empty.length).toBe(0);
  });

  test('discover * returns all agents', async () => {
    const { did: did1 } = generateDID();
    const { did: did2 } = generateDID();

    await registerAgent({
      did: did1,
      name: 'agent-1',
      capabilities: [{ name: 'navigate' }],
      endpoints: {},
    });
    await registerAgent({
      did: did2,
      name: 'agent-2',
      capabilities: [{ name: 'perceive' }],
      endpoints: {},
    });

    const all = await discover('*');
    expect(all.length).toBe(2);
  });

  test('getAgentMetadata returns null for unknown DID', async () => {
    const meta = await getAgentMetadata('did:key:z6MkNOBODY');
    expect(meta).toBeNull();
  });

  test('unregisterAgent removes from registry', async () => {
    const { did } = generateDID();
    await registerAgent({
      did,
      name: 'temp',
      capabilities: [{ name: 'test' }],
      endpoints: {},
    });

    let all = await discover('*');
    expect(all.length).toBe(1);

    await unregisterAgent(did);
    all = await discover('*');
    expect(all.length).toBe(0);
  });

  test('searchAgents by name', async () => {
    const { did } = generateDID();
    await registerAgent({
      did,
      name: 'hiro-robot',
      capabilities: [{ name: 'navigate' }],
      endpoints: {},
    });

    const results = await searchAgents({ name: 'hiro' });
    expect(results).toContain(did);

    const empty = await searchAgents({ name: 'alice' });
    expect(empty.length).toBe(0);
  });
});

describe('A2AServer', () => {
  let server: A2AServer;
  const serverKeypair = generateKeypair();
  const serverDid = publicKeyToDID(serverKeypair.publicKey);

  beforeAll(async () => {
    serverPort = getPort();
    server = new A2AServer({
      port: serverPort,
      did: serverDid,
      privateKey: serverKeypair.privateKey,
      requireAuth: true,
    });

    server.on('echo', async (params) => {
      return { echo: params };
    });

    server.on('add', async (params) => {
      return { sum: params.a + params.b };
    });

    server.on('throws', async () => {
      throw new Error('intentional error');
    });

    await server.start();
  });

  afterAll(async () => {
    await server.stop();
  });

  test('handles authenticated request', async () => {
    const clientKeypair = generateKeypair();
    const clientDid = publicKeyToDID(clientKeypair.publicKey);

    const client = new A2AClient({
      did: clientDid,
      privateKey: clientKeypair.privateKey,
    });

    try {
      const result = await client.request({
        to: `ws://localhost:${serverPort}`,
        method: 'echo',
        params: { hello: 'world' },
        timeout: 5000,
      });

      expect(result.echo).toEqual({ hello: 'world' });
    } finally {
      await client.close();
    }
  });

  test('handles method with computation', async () => {
    const clientKeypair = generateKeypair();
    const clientDid = publicKeyToDID(clientKeypair.publicKey);

    const client = new A2AClient({
      did: clientDid,
      privateKey: clientKeypair.privateKey,
    });

    try {
      const result = await client.request({
        to: `ws://localhost:${serverPort}`,
        method: 'add',
        params: { a: 3, b: 7 },
        timeout: 5000,
      });

      expect(result.sum).toBe(10);
    } finally {
      await client.close();
    }
  });

  test('rejects unauthenticated request', async () => {
    const WebSocket = (await import('ws')).default;
    const ws = new WebSocket(`ws://localhost:${serverPort}`);

    const response = await new Promise<any>((resolve) => {
      ws.on('open', () => {
        ws.send(JSON.stringify({
          jsonrpc: '2.0',
          method: 'echo',
          params: {},
          id: 1,
          // No auth field
        }));
      });

      ws.on('message', (data: Buffer) => {
        resolve(JSON.parse(data.toString()));
        ws.close();
      });
    });

    expect(response.error).toBeDefined();
    expect(response.error.code).toBe(JsonRpcErrorCode.AUTHENTICATION_FAILED);
  });

  test('returns METHOD_NOT_FOUND for unknown methods', async () => {
    const clientKeypair = generateKeypair();
    const clientDid = publicKeyToDID(clientKeypair.publicKey);

    const client = new A2AClient({
      did: clientDid,
      privateKey: clientKeypair.privateKey,
    });

    try {
      await client.request({
        to: `ws://localhost:${serverPort}`,
        method: 'nonexistent',
        params: {},
        timeout: 5000,
      });
      expect(true).toBe(false); // Should not reach here
    } catch (error: any) {
      expect(error.message).toContain(`${JsonRpcErrorCode.METHOD_NOT_FOUND}`);
    } finally {
      await client.close();
    }
  });

  test('returns INTERNAL_ERROR when handler throws', async () => {
    const clientKeypair = generateKeypair();
    const clientDid = publicKeyToDID(clientKeypair.publicKey);

    const client = new A2AClient({
      did: clientDid,
      privateKey: clientKeypair.privateKey,
    });

    try {
      await client.request({
        to: `ws://localhost:${serverPort}`,
        method: 'throws',
        params: {},
        timeout: 5000,
      });
      expect(true).toBe(false);
    } catch (error: any) {
      expect(error.message).toContain('intentional error');
    } finally {
      await client.close();
    }
  });

  test('tracks connected clients', async () => {
    const clientKeypair = generateKeypair();
    const clientDid = publicKeyToDID(clientKeypair.publicKey);

    const client = new A2AClient({
      did: clientDid,
      privateKey: clientKeypair.privateKey,
    });

    try {
      await client.request({
        to: `ws://localhost:${serverPort}`,
        method: 'echo',
        params: {},
        timeout: 5000,
      });

      const clients = server.getConnectedClients();
      expect(clients.length).toBeGreaterThan(0);
    } finally {
      await client.close();
    }
  });
});

describe('A2AServer (no auth)', () => {
  let server: A2AServer;
  let port: number;

  beforeAll(async () => {
    port = getPort();
    server = new A2AServer({
      port,
      did: 'did:key:z6MkSERVER',
      privateKey: new Uint8Array(32),
      requireAuth: false,
    });

    server.on('ping', async () => {
      return { pong: true };
    });

    await server.start();
  });

  afterAll(async () => {
    await server.stop();
  });

  test('accepts request without auth when requireAuth is false', async () => {
    const WebSocket = (await import('ws')).default;
    const ws = new WebSocket(`ws://localhost:${port}`);

    const response = await new Promise<any>((resolve) => {
      ws.on('open', () => {
        ws.send(JSON.stringify({
          jsonrpc: '2.0',
          method: 'ping',
          params: {},
          id: 1,
        }));
      });

      ws.on('message', (data: Buffer) => {
        resolve(JSON.parse(data.toString()));
        ws.close();
      });
    });

    expect(response.result).toEqual({ pong: true });
    expect(response.error).toBeUndefined();
  });
});
