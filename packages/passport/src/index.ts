/**
 * @qualia/passport - Ed25519 identity and passport management
 */

import * as ed25519 from '@noble/ed25519';
import type { Passport } from '@qualia/types';

// Re-export types
export type { Passport, DID, KeyPair } from '@qualia/types';

/**
 * Passport data for signing
 * Contains user-provided fields for passport creation
 */
export interface PassportData {
  capabilities: string[];
}

// Keypair management
export {
  generateKeypair,
  keypairFromPrivateKey,
  isValidPrivateKey,
  isValidPublicKey,
  serializeKeypair,
  deserializeKeypair,
} from './keypair';

// DID operations
export {
  publicKeyToDID,
  didToPublicKey,
  isValidDID,
  parseDID,
} from './did';

// Signing and verification
export {
  sign,
  verify,
  signMessage,
  verifyMessage,
} from './signature';

// Passport creation, verification, rotation
export {
  createPassport,
  verifyPassport,
  serializePassport,
  deserializePassport,
  batchVerify,
  isExpired,
  createKeyRotationProof,
  verifyKeyRotationProof,
  rotatePassport,
} from './passport';
export type {
  CreatePassportOptions,
  VerifyPassportOptions,
  KeyRotationProof,
} from './passport';

// Encoding utilities
export { toHex, fromHex } from './utils';

// Passport storage
export {
  MemoryPassportStore,
  FilePassportStore,
} from './store';
export type { PassportStore } from './store';

// ============================================================================
// Convenience API
// ============================================================================

import { generateKeypair } from './keypair';
import { publicKeyToDID } from './did';
import { createPassport } from './passport';

/**
 * Generate DID from Ed25519 keypair
 * @returns Object with DID and private key
 */
export function generateDID(): { did: string; privateKey: Uint8Array } {
  const keypair = generateKeypair();
  const did = publicKeyToDID(keypair.publicKey);

  return {
    did,
    privateKey: keypair.privateKey,
  };
}

/**
 * Sign a passport
 * @param data - Passport data (must include capabilities array)
 * @param privateKey - Ed25519 private key
 * @returns Signed passport
 */
export async function signPassport(
  data: PassportData,
  privateKey: Uint8Array
): Promise<Passport> {
  // Derive public key from private key
  const publicKey = ed25519.getPublicKey(privateKey);

  const keypair = {
    publicKey,
    privateKey,
  };

  return createPassport(keypair, data.capabilities);
}
