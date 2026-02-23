/**
 * JSON-RPC 2.0 types for agent-to-agent communication
 */

export interface JsonRpcRequest {
  jsonrpc: '2.0';
  method: string;
  params?: any;
  auth?: {
    from: string; // DID
    signature: string;
  };
  id: string | number;
}

export interface JsonRpcResponse {
  jsonrpc: '2.0';
  result?: any;
  error?: JsonRpcError;
  id: string | number;
}

export interface JsonRpcError {
  code: number;
  message: string;
  data?: any;
}

export interface A2AServerConfig {
  port: number;
  did: string;
  privateKey: Uint8Array;
  host?: string;
  /** If false, skip DID signature verification (for local/trusted networks) */
  requireAuth?: boolean;
}

export interface A2AClientConfig {
  did: string;
  privateKey: Uint8Array;
}

export interface RequestParams {
  to: string; // Target DID, NANDA name, or WebSocket URL
  method: string;
  params: any;
  timeout?: number;
}

export type MethodHandler = (params: any, from: string) => Promise<any>;

/**
 * Standard JSON-RPC error codes
 */
export enum JsonRpcErrorCode {
  PARSE_ERROR = -32700,
  INVALID_REQUEST = -32600,
  METHOD_NOT_FOUND = -32601,
  INVALID_PARAMS = -32602,
  INTERNAL_ERROR = -32603,
  // Custom error codes
  AUTHENTICATION_FAILED = -32000,
  TIMEOUT = -32001,
  DISCOVERY_FAILED = -32002,
}
