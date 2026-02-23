/**
 * @qualia/passport - Ed25519 identity and passport management
 */

import * as ed25519 from '@noble/ed25519';
import type { Passport } from '@qualia/types';

// Re-export types
export type { Passport, DID, KeyPair } from '@qualia/types';

/**
 * Passport data for signing
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

// Passport creation and verification
export {
  createPassport,
  verifyPassport,
  serializePassport,
  deserializePassport,
} from './passport';

// Hex encoding utilities
export { toHex, fromHex } from './utils';

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
  const publicKey = ed25519.getPublicKey(privateKey);

  const keypair = {
    publicKey,
    privateKey,
  };

  return createPassport(keypair, data.capabilities);
}
