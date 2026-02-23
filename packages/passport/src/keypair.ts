/**
 * Ed25519 keypair generation and management
 */

import * as ed25519 from '@noble/ed25519';
import type { KeyPair } from '@qualia/types';
import { toHex, fromHex } from './utils';

// Configure SHA512 for @noble/ed25519 (required for sync operations)
if (typeof Bun !== 'undefined') {
  ed25519.etc.sha512Sync = (...m) => {
    const hasher = new Bun.CryptoHasher('sha512');
    hasher.update(ed25519.etc.concatBytes(...m));
    return new Uint8Array(hasher.digest());
  };
}

/**
 * Generate a new Ed25519 keypair
 * @returns KeyPair with 32-byte public and private keys
 */
export function generateKeypair(): KeyPair {
  const privateKey = ed25519.utils.randomPrivateKey();
  const publicKey = ed25519.getPublicKey(privateKey);

  return {
    publicKey,
    privateKey,
  };
}

/**
 * Derive public key from private key
 * @param privateKey - Ed25519 private key (32 bytes)
 * @returns KeyPair with derived public key
 * @throws Error if private key is invalid
 */
export function keypairFromPrivateKey(privateKey: Uint8Array): KeyPair {
  if (!isValidPrivateKey(privateKey)) {
    throw new Error('Invalid private key: must be 32 non-zero bytes');
  }

  const publicKey = ed25519.getPublicKey(privateKey);

  return {
    publicKey,
    privateKey,
  };
}

/**
 * Validate private key format
 * @param privateKey - Key to validate
 * @returns True if key is valid
 */
export function isValidPrivateKey(privateKey: any): boolean {
  if (!privateKey || !(privateKey instanceof Uint8Array)) {
    return false;
  }

  if (privateKey.length !== 32) {
    return false;
  }

  // Check that key is not all zeros
  const sum = privateKey.reduce((a, b) => a + b, 0);
  if (sum === 0) {
    return false;
  }

  // Validate using @noble/ed25519
  try {
    ed25519.getPublicKey(privateKey);
    return true;
  } catch {
    return false;
  }
}

/**
 * Validate public key format
 * @param publicKey - Key to validate
 * @returns True if key is valid
 */
export function isValidPublicKey(publicKey: any): boolean {
  if (!publicKey || !(publicKey instanceof Uint8Array)) {
    return false;
  }

  if (publicKey.length !== 32) {
    return false;
  }

  // Check that key is not all zeros
  const sum = publicKey.reduce((a, b) => a + b, 0);
  if (sum === 0) {
    return false;
  }

  return true;
}

/**
 * Serialize keypair to hex strings
 * @param keypair - KeyPair to serialize
 * @returns Object with hex-encoded keys
 */
export function serializeKeypair(keypair: KeyPair): {
  publicKey: string;
  privateKey: string;
} {
  return {
    publicKey: toHex(keypair.publicKey),
    privateKey: toHex(keypair.privateKey),
  };
}

/**
 * Deserialize keypair from hex strings
 * @param serialized - Object with hex-encoded keys
 * @returns KeyPair
 * @throws Error if hex strings are invalid
 */
export function deserializeKeypair(serialized: {
  publicKey: string;
  privateKey: string;
}): KeyPair {
  try {
    const publicKey = fromHex(serialized.publicKey);
    const privateKey = fromHex(serialized.privateKey);

    if (publicKey.length !== 32 || privateKey.length !== 32) {
      throw new Error('Invalid key length after deserialization');
    }

    if (!isValidPublicKey(publicKey) || !isValidPrivateKey(privateKey)) {
      throw new Error('Invalid keys after deserialization');
    }

    return { publicKey, privateKey };
  } catch (error) {
    throw new Error(
      `Failed to deserialize keypair: ${error instanceof Error ? error.message : 'Unknown error'}`
    );
  }
}
