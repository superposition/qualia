/**
 * Tests for pluggable discovery provider
 */

import { InMemoryDiscovery } from '../src/discovery';
import type { AgentMetadata } from '../src/discovery';

describe('InMemoryDiscovery', () => {
  let discovery: InMemoryDiscovery;

  beforeEach(() => {
    discovery = new InMemoryDiscovery();
  });

  it('should start empty', async () => {
    const agents = await discovery.discover('*');
    expect(agents).toEqual([]);
  });

  it('should register and discover agents', async () => {
    const agent: AgentMetadata = {
      did: 'did:key:z6MkTestAgent1',
      name: 'test-agent',
      capabilities: [{ name: 'navigate' }],
      endpoints: { a2a: 'ws://localhost:9000' },
    };

    await discovery.registerAgent(agent);
    const found = await discovery.discover('navigate');

    expect(found).toContain(agent.did);
  });

  it('should discover all agents with wildcard', async () => {
    await discovery.registerAgent({
      did: 'did:key:z6MkA1',
      name: 'a1',
      capabilities: [{ name: 'a' }],
      endpoints: {},
    });
    await discovery.registerAgent({
      did: 'did:key:z6MkA2',
      name: 'a2',
      capabilities: [{ name: 'b' }],
      endpoints: {},
    });

    const all = await discovery.discover('*');
    expect(all.length).toBe(2);
  });

  it('should return empty for unknown capability', async () => {
    await discovery.registerAgent({
      did: 'did:key:z6MkA1',
      name: 'a1',
      capabilities: [{ name: 'x' }],
      endpoints: {},
    });

    const found = await discovery.discover('nonexistent');
    expect(found).toEqual([]);
  });

  it('should get agent metadata by DID', async () => {
    const agent: AgentMetadata = {
      did: 'did:key:z6MkMeta',
      name: 'meta-agent',
      capabilities: [{ name: 'perceive' }],
      endpoints: { a2a: 'ws://localhost:9001' },
    };

    await discovery.registerAgent(agent);
    const meta = await discovery.getAgentMetadata('did:key:z6MkMeta');

    expect(meta).not.toBeNull();
    expect(meta!.name).toBe('meta-agent');
  });

  it('should return null for unknown DID', async () => {
    const meta = await discovery.getAgentMetadata('did:key:z6MkUnknown');
    expect(meta).toBeNull();
  });

  it('should search by name (case insensitive)', async () => {
    await discovery.registerAgent({
      did: 'did:key:z6MkAlice',
      name: 'Alice-Agent',
      capabilities: [],
      endpoints: {},
    });

    const found = await discovery.searchAgents({ name: 'alice' });
    expect(found).toContain('did:key:z6MkAlice');
  });

  it('should search by capabilities', async () => {
    await discovery.registerAgent({
      did: 'did:key:z6MkNav',
      name: 'nav',
      capabilities: [{ name: 'navigate' }, { name: 'map' }],
      endpoints: {},
    });
    await discovery.registerAgent({
      did: 'did:key:z6MkSensor',
      name: 'sensor',
      capabilities: [{ name: 'perceive' }],
      endpoints: {},
    });

    const found = await discovery.searchAgents({ capabilities: ['navigate'] });
    expect(found).toContain('did:key:z6MkNav');
    expect(found).not.toContain('did:key:z6MkSensor');
  });

  it('should update existing agent on re-register', async () => {
    await discovery.registerAgent({
      did: 'did:key:z6MkUpdate',
      name: 'v1',
      capabilities: [],
      endpoints: {},
    });
    await discovery.registerAgent({
      did: 'did:key:z6MkUpdate',
      name: 'v2',
      capabilities: [{ name: 'new-cap' }],
      endpoints: {},
    });

    const meta = await discovery.getAgentMetadata('did:key:z6MkUpdate');
    expect(meta!.name).toBe('v2');
    expect(meta!.capabilities.length).toBe(1);

    const all = await discovery.discover('*');
    expect(all.length).toBe(1);
  });

  it('should clear the registry', async () => {
    await discovery.registerAgent({
      did: 'did:key:z6MkClear',
      name: 'clear',
      capabilities: [],
      endpoints: {},
    });

    discovery.clear();
    const all = await discovery.discover('*');
    expect(all).toEqual([]);
  });

  it('should initialize with agents', async () => {
    const preloaded = new InMemoryDiscovery([
      { did: 'did:key:z6MkPre1', name: 'pre1', capabilities: [], endpoints: {} },
      { did: 'did:key:z6MkPre2', name: 'pre2', capabilities: [], endpoints: {} },
    ]);

    const all = await preloaded.discover('*');
    expect(all.length).toBe(2);
  });
});
