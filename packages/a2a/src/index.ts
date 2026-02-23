import type { A2AMessage, A2ARequest, A2AResponse } from '@qualia/types';

export { type A2AMessage, type A2ARequest, type A2AResponse };

// Re-export client and server
export { A2AClient } from './client';
export type { ClientEvent, ClientEventListener, ReconnectConfig } from './client';
export { A2AServer } from './server';
export type { ServerEvent, ServerEventListener, HeartbeatConfig } from './server';
export * from './types';

// Middleware
export {
  composeMiddleware,
  rateLimiter,
  logger,
} from './middleware';
export type { Middleware, MiddlewareContext, NextFunction } from './middleware';

// Discovery
export {
  InMemoryDiscovery,
  discover,
  getAgentMetadata,
  registerAgent,
  unregisterAgent,
  searchAgents,
  getMockRegistry,
  clearMockRegistry,
  clearMockRegistry as clearRegistry,
} from './discovery';
export type { DiscoveryProvider, AgentMetadata, AgentCapability, SearchCriteria } from './discovery';

// Re-export mock passport utilities for testing
export {
  generateMockIdentity,
  signRequest,
  verifyPassport,
} from './__mocks__/passport';
