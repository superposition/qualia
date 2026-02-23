/**
 * @qualia/a2a - Agent-to-Agent JSON-RPC protocol with DID authentication
 */

import type { A2AMessage, A2ARequest, A2AResponse } from '@qualia/types';

export { type A2AMessage, type A2ARequest, type A2AResponse };

// Client and server
export { A2AClient } from './client';
export { A2AServer } from './server';

// Types
export * from './types';

// Discovery
export {
  discover,
  getAgentMetadata,
  registerAgent,
  unregisterAgent,
  searchAgents,
  getRegistry,
  clearRegistry,
} from './discovery';

export type {
  AgentCapability,
  AgentMetadata,
  SearchCriteria,
} from './discovery';
