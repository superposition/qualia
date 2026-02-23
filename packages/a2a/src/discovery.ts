/**
 * Agent discovery
 *
 * Provides in-memory agent registry for local development and testing.
 * In production, plug in on-chain or mDNS-based discovery.
 */

export interface AgentCapability {
  name: string;
  version?: string;
  description?: string;
}

export interface AgentMetadata {
  did: string;
  name: string;
  capabilities: AgentCapability[];
  endpoints: {
    a2a?: string; // WebSocket endpoint for A2A communication
    http?: string; // HTTP endpoint (if available)
  };
}

/**
 * In-memory agent registry for development and testing
 */
const agentRegistry: AgentMetadata[] = [];

/**
 * Discover agents by capability
 * @param capability - The capability name to search for, or '*' for all agents
 * @returns Array of agent DIDs
 */
export async function discover(capability: string): Promise<string[]> {
  if (capability === '*') {
    return agentRegistry.map((agent) => agent.did);
  }

  const matchingAgents = agentRegistry.filter((agent) =>
    agent.capabilities.some((cap) => cap.name === capability)
  );

  return matchingAgents.map((agent) => agent.did);
}

/**
 * Get full agent metadata by DID
 * @param did - The agent's DID
 * @returns Agent metadata or null if not found
 */
export async function getAgentMetadata(
  did: string
): Promise<AgentMetadata | null> {
  const agent = agentRegistry.find((a) => a.did === did);
  return agent || null;
}

/**
 * Register an agent in the discovery registry
 * @param metadata - Agent metadata to register
 */
export async function registerAgent(metadata: AgentMetadata): Promise<void> {
  // Remove existing entry if present (update)
  const idx = agentRegistry.findIndex((a) => a.did === metadata.did);
  if (idx >= 0) {
    agentRegistry[idx] = metadata;
  } else {
    agentRegistry.push(metadata);
  }
}

/**
 * Remove an agent from the registry
 * @param did - The agent's DID to remove
 */
export async function unregisterAgent(did: string): Promise<void> {
  const idx = agentRegistry.findIndex((a) => a.did === did);
  if (idx >= 0) {
    agentRegistry.splice(idx, 1);
  }
}

export interface SearchCriteria {
  capabilities?: string[];
  name?: string;
}

/**
 * Advanced agent search with multiple criteria
 * @param criteria - Search criteria
 * @returns Array of matching agent DIDs
 */
export async function searchAgents(
  criteria: SearchCriteria
): Promise<string[]> {
  let results = agentRegistry;

  if (criteria.capabilities && criteria.capabilities.length > 0) {
    results = results.filter((agent) =>
      criteria.capabilities!.some((cap) =>
        agent.capabilities.some((agentCap) => agentCap.name === cap)
      )
    );
  }

  if (criteria.name) {
    results = results.filter((agent) =>
      agent.name.toLowerCase().includes(criteria.name!.toLowerCase())
    );
  }

  return results.map((agent) => agent.did);
}

/**
 * Get a copy of the registry (for testing)
 */
export function getRegistry(): AgentMetadata[] {
  return [...agentRegistry];
}

/**
 * Clear the registry (for testing)
 */
export function clearRegistry(): void {
  agentRegistry.length = 0;
}
