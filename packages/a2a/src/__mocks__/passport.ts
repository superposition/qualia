/**
 * Mock DID-based passport authentication for Phase 1 development
 *
 * This is a simplified mock implementation to allow parallel development
 * while the @qualia/passport package is being built.
 *
 * Phase 1 (Current): Simple signature verification mock
 * Phase 2 (Future): Integration with real @qualia/passport using Ed25519
 *
 * Real implementation will:
 * 1. Verify Ed25519 signature using agent's public key
 * 2. Resolve DID to public key via did:key method
 * 3. Verify the signature matches the request payload
 * 4. Check passport validity and revocation status
 */

import { createHash } from 'crypto';

/**
 * Sign a request payload with private key
 * @param privateKey - Agent's private key
 * @param payload - Request payload to sign
 * @returns Promise that resolves with signature string
 */
export async function signRequest(
  privateKey: Uint8Array,
  payload: any
): Promise<string> {
  // Mock implementation - in production, this uses Ed25519 signing
  // with the agent's private key from their passport

  // For now, create a deterministic signature based on payload
  const payloadStr = JSON.stringify(payload);
  const keyStr = Buffer.from(privateKey).toString('hex');

  const hash = createHash('sha256')
    .update(payloadStr)
    .update(keyStr)
    .digest('hex');

  return `mock-sig-${hash.substring(0, 32)}`;
}

/**
 * Verify a passport signature
 * @param did - The DID of the signing agent
 * @param signature - The signature to verify
 * @param payload - The signed payload
 * @returns Promise that resolves with verification result
 */
export async function verifyPassport(
  did: string,
  signature: string,
  _payload: any
): Promise<boolean> {
  // Mock implementation - in production, this:
  // 1. Resolves DID to public key using did:key method
  // 2. Verifies Ed25519 signature
  // 3. Checks passport validity

  // For Phase 1, we accept any signature that starts with "mock-sig-"
  // This allows testing the protocol flow without blocking on crypto implementation

  if (!did.startsWith('did:key:')) {
    console.warn(`Invalid DID format: ${did}`);
    return false;
  }

  if (!signature.startsWith('mock-sig-')) {
    console.warn(`Invalid signature format: ${signature}`);
    return false;
  }

  // In Phase 1, all properly formatted signatures are valid
  return true;
}

/**
 * Create a mock passport for testing
 * @param did - Agent DID
 * @returns Mock passport object
 */
export function createMockPassport(did: string) {
  return {
    did,
    name: `agent-${did.substring(8, 16)}`,
    publicKey: new Uint8Array(32).fill(1), // Mock public key
    capabilities: ['test'],
    created: new Date().toISOString(),
  };
}

/**
 * Generate a mock DID and keypair for testing
 * @returns Object with DID and private key
 */
export function generateMockIdentity(): {
  did: string;
  privateKey: Uint8Array;
} {
  // Generate a random "private key" for testing
  const privateKey = new Uint8Array(32);
  for (let i = 0; i < 32; i++) {
    privateKey[i] = Math.floor(Math.random() * 256);
  }

  // Create a mock DID from the "public key"
  const publicKeyHash = createHash('sha256')
    .update(Buffer.from(privateKey))
    .digest('hex')
    .substring(0, 32);

  const did = `did:key:z6Mk${publicKeyHash}`;

  return { did, privateKey };
}

/**
 * Validate DID format
 * @param did - DID string to validate
 * @returns True if DID format is valid
 */
export function isValidDID(did: string): boolean {
  return did.startsWith('did:key:z6Mk') && did.length >= 48;
}
