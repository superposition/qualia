/**
 * Swarm coordination types
 */

/** Swarm message wrapper */
export interface SwarmMessage<T = unknown> {
  /** Message type */
  type: SwarmMessageType;
  /** Sender agent DID */
  from: string;
  /** Message payload */
  data: T;
  /** Unix timestamp in ms */
  timestamp: number;
}

/** Swarm message types */
export type SwarmMessageType =
  | 'position_update'
  | 'area_claim'
  | 'area_release'
  | 'alert'
  | 'task_proposal'
  | 'task_bid'
  | 'task_award'
  | 'heartbeat';

/** Agent position report */
export interface AgentPosition {
  /** Agent DID */
  did: string;
  /** Current position */
  x: number;
  y: number;
  /** Heading (radians) */
  theta: number;
  /** Unix timestamp in ms */
  timestamp: number;
}

/** Area claim for spatial deconfliction */
export interface AreaClaim {
  /** Claim ID */
  id: string;
  /** Claiming agent DID */
  agentDid: string;
  /** Center of claimed area */
  center: { x: number; y: number };
  /** Radius of claimed area (meters) */
  radius: number;
  /** When the claim expires (Unix ms) */
  expiresAt: number;
}

/** Alert types for fleet communication */
export type AlertType = 'obstacle' | 'danger' | 'discovery' | 'low_battery' | 'stuck';

/** Fleet alert */
export interface FleetAlert {
  /** Alert type */
  type: AlertType;
  /** Location of the alert */
  location: { x: number; y: number };
  /** Severity 1-5 */
  severity: number;
  /** Human-readable message */
  message: string;
}

/** Task proposal for auction */
export interface TaskProposal {
  /** Task ID */
  id: string;
  /** Task description */
  description: string;
  /** Required capabilities */
  requiredCapabilities: string[];
  /** Target location */
  location?: { x: number; y: number };
  /** Deadline (Unix ms) */
  deadline?: number;
}

/** Task bid from an agent */
export interface TaskBid {
  /** Task ID being bid on */
  taskId: string;
  /** Bidding agent DID */
  agentDid: string;
  /** Estimated cost (lower = better) */
  cost: number;
  /** Estimated time to complete (ms) */
  estimatedTime: number;
}
