/**
 * DID (Decentralized Identifier) generation and parsing
 * Implements did:key method with Ed25519 public keys
 */

import { base58 } from '@scure/base';
import type { DID } from '@qualia/types';

// Multicodec prefix for Ed25519 public key (0xed 0x01)
const ED25519_MULTICODEC_PREFIX = new Uint8Array([0xed, 0x01]);

/**
 * Convert Ed25519 public key to DID
 * @param publicKey - Ed25519 public key (32 bytes)
 * @returns DID in format did:key:z6Mk...
 * @throws Error if public key is invalid
 */
export function publicKeyToDID(publicKey: Uint8Array): DID {
  if (publicKey.length !== 32) {
    throw new Error('Public key must be 32 bytes');
  }

  // Prepend multicodec prefix
  const multicodecKey = new Uint8Array(
    ED25519_MULTICODEC_PREFIX.length + publicKey.length
  );
  multicodecKey.set(ED25519_MULTICODEC_PREFIX, 0);
  multicodecKey.set(publicKey, ED25519_MULTICODEC_PREFIX.length);

  // Encode to base58btc (multibase 'z' prefix)
  const base58Encoded = base58.encode(multicodecKey);

  return `did:key:z${base58Encoded}` as DID;
}

/**
 * Extract public key from DID
 * @param did - DID string
 * @returns Ed25519 public key (32 bytes)
 * @throws Error if DID is invalid or not did:key method
 */
export function didToPublicKey(did: DID): Uint8Array {
  if (!isValidDID(did)) {
    throw new Error('Invalid DID format');
  }

  // Remove 'did:key:' prefix
  const methodSpecificId = did.slice(8);

  // Check multibase prefix
  if (!methodSpecificId.startsWith('z')) {
    throw new Error('DID must use base58btc encoding (z prefix)');
  }

  try {
    // Decode base58
    const multicodecKey = base58.decode(methodSpecificId.slice(1));

    // Verify multicodec prefix
    if (
      multicodecKey.length < ED25519_MULTICODEC_PREFIX.length ||
      multicodecKey[0] !== ED25519_MULTICODEC_PREFIX[0] ||
      multicodecKey[1] !== ED25519_MULTICODEC_PREFIX[1]
    ) {
      throw new Error('DID does not contain Ed25519 public key (wrong multicodec)');
    }

    // Extract public key (remove multicodec prefix)
    const publicKey = multicodecKey.slice(ED25519_MULTICODEC_PREFIX.length);

    if (publicKey.length !== 32) {
      throw new Error('Public key must be 32 bytes');
    }

    return publicKey;
  } catch (error) {
    throw new Error(
      `Failed to extract public key from DID: ${error instanceof Error ? error.message : 'Unknown error'}`
    );
  }
}

/**
 * Validate DID format
 * @param did - DID string to validate
 * @returns True if DID is valid did:key format
 */
export function isValidDID(did: any): boolean {
  if (typeof did !== 'string') {
    return false;
  }

  if (!did.startsWith('did:key:z')) {
    return false;
  }

  // Check minimum length (did:key:z + base58 encoded 34 bytes)
  if (did.length < 48) {
    return false;
  }

  // Check that it only contains valid base58 characters after the prefix
  const methodSpecificId = did.slice(9); // Skip 'did:key:z'
  const base58Regex = /^[1-9A-HJ-NP-Za-km-z]+$/;

  if (!base58Regex.test(methodSpecificId)) {
    return false;
  }

  return true;
}

/**
 * Parse DID into components
 * @param did - DID string
 * @returns Object with DID components
 * @throws Error if DID is invalid
 */
export function parseDID(did: DID): {
  method: string;
  id: string;
  publicKey: Uint8Array;
} {
  if (!isValidDID(did)) {
    throw new Error('Invalid DID format');
  }

  const parts = did.split(':');
  const method = parts[1] as string;
  const id = parts.slice(2).join(':');

  const publicKey = didToPublicKey(did);

  return {
    method,
    id,
    publicKey,
  };
}
