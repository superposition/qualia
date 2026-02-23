/**
 * Pluggable agent discovery
 */

/** Agent capability metadata */
export interface AgentCapability {
  name: string;
  version?: string;
  description?: string;
}

/** Agent metadata for discovery */
export interface AgentMetadata {
  did: string;
  name: string;
  capabilities: AgentCapability[];
  endpoints: {
    a2a?: string;
    http?: string;
  };
}

/** Search criteria for finding agents */
export interface SearchCriteria {
  capabilities?: string[];
  name?: string;
}

/**
 * Discovery provider interface — implement this to plug in custom discovery
 */
export interface DiscoveryProvider {
  discover(capability: string): Promise<string[]>;
  getAgentMetadata(did: string): Promise<AgentMetadata | null>;
  registerAgent(metadata: AgentMetadata): Promise<void>;
  searchAgents(criteria: SearchCriteria): Promise<string[]>;
}

/**
 * In-memory discovery provider — default implementation
 */
export class InMemoryDiscovery implements DiscoveryProvider {
  private registry: AgentMetadata[] = [];

  constructor(initialAgents?: AgentMetadata[]) {
    if (initialAgents) {
      this.registry = [...initialAgents];
    }
  }

  async discover(capability: string): Promise<string[]> {
    if (capability === '*') {
      return this.registry.map((agent) => agent.did);
    }

    return this.registry
      .filter((agent) =>
        agent.capabilities.some((cap) => cap.name === capability),
      )
      .map((agent) => agent.did);
  }

  async getAgentMetadata(did: string): Promise<AgentMetadata | null> {
    return this.registry.find((a) => a.did === did) ?? null;
  }

  async registerAgent(metadata: AgentMetadata): Promise<void> {
    const existing = this.registry.findIndex((a) => a.did === metadata.did);
    if (existing >= 0) {
      this.registry[existing] = metadata;
    } else {
      this.registry.push(metadata);
    }
  }

  async unregisterAgent(did: string): Promise<boolean> {
    const index = this.registry.findIndex((a) => a.did === did);
    if (index >= 0) {
      this.registry.splice(index, 1);
      return true;
    }
    return false;
  }

  async searchAgents(criteria: SearchCriteria): Promise<string[]> {
    let results = this.registry;

    if (criteria.capabilities && criteria.capabilities.length > 0) {
      results = results.filter((agent) =>
        criteria.capabilities!.some((cap) =>
          agent.capabilities.some((agentCap) => agentCap.name === cap),
        ),
      );
    }

    if (criteria.name) {
      const searchName = criteria.name.toLowerCase();
      results = results.filter((agent) =>
        agent.name.toLowerCase().includes(searchName),
      );
    }

    return results.map((agent) => agent.did);
  }

  /** Get a copy of the registry (for testing) */
  getRegistry(): AgentMetadata[] {
    return [...this.registry];
  }

  /** Clear the registry */
  clear(): void {
    this.registry.length = 0;
  }
}

// ============================================================================
// Default instance with mock data (backward compatible)
// ============================================================================

const defaultDiscovery = new InMemoryDiscovery([
  {
    did: 'did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK',
    name: 'alice-agent',
    capabilities: [
      { name: 'obstacle_analysis', version: '1.0' },
      { name: 'image_processing', version: '1.0' },
    ],
    endpoints: {
      a2a: 'ws://localhost:8080',
      http: 'http://localhost:8080',
    },
  },
  {
    did: 'did:key:z6MkkZc5xKBZmNJvMBTMp7kGLjBvHN3qvEsCqcV2p5xKx8uK',
    name: 'bob-agent',
    capabilities: [
      { name: 'data_analysis', version: '1.0' },
      { name: 'prediction', version: '1.0' },
    ],
    endpoints: {
      a2a: 'ws://localhost:8081',
    },
  },
  {
    did: 'did:key:z6MkvYzpqzUTfRq3RdCYqNS8KKxX2ZwMhKz6f6g8VjKqGc4R',
    name: 'hiro-robot',
    capabilities: [
      { name: 'navigation', version: '1.0' },
      { name: 'sensor_fusion', version: '1.0' },
    ],
    endpoints: {
      a2a: 'ws://localhost:8082',
    },
  },
]);

/** Discover agents by capability (uses default discovery) */
export async function discover(capability: string): Promise<string[]> {
  return defaultDiscovery.discover(capability);
}

/** Get agent metadata (uses default discovery) */
export async function getAgentMetadata(
  did: string,
): Promise<AgentMetadata | null> {
  return defaultDiscovery.getAgentMetadata(did);
}

/** Register agent (uses default discovery) */
export async function registerAgent(metadata: AgentMetadata): Promise<void> {
  return defaultDiscovery.registerAgent(metadata);
}

/** Search agents (uses default discovery) */
export async function searchAgents(criteria: SearchCriteria): Promise<string[]> {
  return defaultDiscovery.searchAgents(criteria);
}

/** Unregister agent (uses default discovery) */
export async function unregisterAgent(did: string): Promise<boolean> {
  return defaultDiscovery.unregisterAgent(did);
}

/** Get mock registry (for testing) */
export function getMockRegistry(): AgentMetadata[] {
  return defaultDiscovery.getRegistry();
}

/** Clear mock registry (for testing) */
export function clearMockRegistry(): void {
  defaultDiscovery.clear();
}
