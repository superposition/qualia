/**
 * QUALIA Shared Type Definitions
 *
 * Core types used across all packages in the QUALIA robotics framework.
 */

// ============================================================================
// Module re-exports (for tree-shaking)
// ============================================================================

export * from './ros';
export * from './events';
export * from './motion';
export * from './grid';
export * from './swarm';
export * from './sensor';

// ============================================================================
// NANDA (Namespace-Addressable Namespace for Decentralized Agents)
// ============================================================================

/**
 * NANDA name format: @entity:namespace
 * Examples: @hiro:qualia, @alice:anthropic
 */
export type NANDAName = `@${string}:${string}`;

/**
 * Decentralized Identifier (DID)
 * Format: did:key:z6Mk...
 */
export type DID = `did:key:z${string}`;

/**
 * Ethereum address (0x + 40 hex characters)
 */
export type Address = `0x${string}`;

/**
 * NANDA registry entry for an agent or robot
 */
export interface NANDA {
  /** NANDA name (@entity:namespace) */
  name: NANDAName;

  /** Decentralized identifier (did:key:...) */
  did: DID;

  /** List of capabilities (e.g., ["navigate", "perceive", "reason"]) */
  capabilities: string[];

  /** Ethereum wallet address */
  wallet: Address;

  /** Whether the entity is currently active */
  isActive?: boolean;

  /** Timestamp when registered (Unix timestamp in seconds) */
  registeredAt?: number;
}

// ============================================================================
// Passport & Identity
// ============================================================================

/**
 * Digital passport for agent/robot identity
 */
export interface Passport {
  /** Decentralized identifier */
  did: DID;

  /** Ed25519 public key (hex-encoded) */
  publicKey: string;

  /** Agent capabilities */
  capabilities: string[];

  /** Signature proving ownership of the DID */
  signature: string;

  /** Timestamp when passport was issued (Unix timestamp in seconds) */
  issuedAt: number;

  /** Optional expiration timestamp (Unix timestamp in seconds) */
  expiresAt?: number;
}

/**
 * Ed25519 key pair
 */
export interface KeyPair {
  publicKey: Uint8Array;
  privateKey: Uint8Array;
}

// ============================================================================
// Agent-to-Agent (A2A) Protocol
// ============================================================================

/**
 * A2A JSON-RPC request
 */
export interface A2ARequest {
  jsonrpc: '2.0';
  id: string | number;
  method: string;
  params?: unknown;
}

/**
 * A2A JSON-RPC response
 */
export interface A2AResponse {
  jsonrpc: '2.0';
  id: string | number;
  result?: unknown;
  error?: A2AError;
}

/**
 * A2A error object
 */
export interface A2AError {
  code: number;
  message: string;
  data?: unknown;
}

/**
 * A2A message envelope
 */
export interface A2AMessage {
  from: DID;
  to: DID;
  request: A2ARequest;
  signature: string;
  timestamp: number;
}

// ============================================================================
// Robot & Agent Capabilities
// ============================================================================

/**
 * Standard capability categories
 */
export type CapabilityCategory =
  | 'navigate'    // Movement and navigation
  | 'perceive'    // Sensing and observation
  | 'reason'      // Decision-making and planning
  | 'manipulate'  // Physical manipulation
  | 'communicate' // Communication and interaction
  | 'learn';      // Learning and adaptation

/**
 * Capability definition
 */
export interface Capability {
  name: string;
  category: CapabilityCategory;
  description: string;
  parameters?: Record<string, unknown>;
}

// ============================================================================
// Monitoring & Telemetry
// ============================================================================

/**
 * System health status
 */
export type HealthStatus = 'healthy' | 'degraded' | 'unhealthy';

/**
 * Health check result
 */
export interface HealthCheck {
  status: HealthStatus;
  timestamp: number;
  checks: Record<string, boolean>;
  error?: string;
}

/**
 * Metric data point
 */
export interface Metric {
  name: string;
  value: number;
  timestamp: number;
  tags?: Record<string, string>;
}

// ============================================================================
// Utility Types
// ============================================================================

/**
 * Result type for operations that can fail
 */
export type Result<T, E = Error> =
  | { ok: true; value: T }
  | { ok: false; error: E };

/**
 * Optional timestamp fields
 */
export interface Timestamped {
  createdAt: number;
  updatedAt?: number;
}

/**
 * Entity with ID
 */
export interface WithId<T = string> {
  id: T;
}
