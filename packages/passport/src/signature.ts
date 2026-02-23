/**
 * Ed25519 message signing and verification
 */

import * as ed25519 from '@noble/ed25519';
import canonicalize from 'canonicalize';
import { didToPublicKey } from './did';
import { toHex, fromHex } from './utils';
import type { DID } from '@qualia/types';

/**
 * Sign a message with Ed25519 private key
 * @param message - Message to sign (arbitrary bytes)
 * @param privateKey - Ed25519 private key (32 bytes)
 * @returns Signature (64 bytes)
 * @throws Error if private key is invalid
 */
export async function sign(
  message: Uint8Array,
  privateKey: Uint8Array
): Promise<Uint8Array> {
  if (privateKey.length !== 32) {
    throw new Error('Private key must be 32 bytes');
  }

  try {
    const signature = ed25519.sign(message, privateKey);
    return signature;
  } catch (error) {
    throw new Error(
      `Failed to sign message: ${error instanceof Error ? error.message : 'Unknown error'}`
    );
  }
}

/**
 * Verify Ed25519 signature
 * @param signature - Signature to verify (64 bytes)
 * @param message - Original message
 * @param publicKey - Ed25519 public key (32 bytes)
 * @returns True if signature is valid
 */
export async function verify(
  signature: Uint8Array,
  message: Uint8Array,
  publicKey: Uint8Array
): Promise<boolean> {
  if (signature.length !== 64) {
    return false;
  }

  if (publicKey.length !== 32) {
    return false;
  }

  try {
    return ed25519.verify(signature, message, publicKey);
  } catch (error) {
    return false;
  }
}

/**
 * Sign a JSON payload (for A2A protocol)
 * @param payload - JSON-serializable object
 * @param privateKey - Ed25519 private key (32 bytes)
 * @returns Hex-encoded signature (128 hex characters)
 */
export async function signMessage(
  payload: any,
  privateKey: Uint8Array
): Promise<string> {
  const message = canonicalize(payload);
  if (!message) {
    throw new Error('Failed to canonicalize payload');
  }
  const messageBytes = new TextEncoder().encode(message);

  const signature = await sign(messageBytes, privateKey);

  return toHex(signature);
}

/**
 * Verify a JSON payload signature
 * @param did - DID of the signer
 * @param signature - Hex-encoded signature
 * @param payload - JSON-serializable object
 * @returns True if signature is valid
 */
export async function verifyMessage(
  did: DID,
  signature: string,
  payload: any
): Promise<boolean> {
  try {
    const publicKey = didToPublicKey(did);

    if (!signature || typeof signature !== 'string') {
      return false;
    }

    if (signature.length !== 128 || !/^[0-9a-f]{128}$/.test(signature)) {
      return false;
    }

    const signatureBytes = fromHex(signature);

    const message = canonicalize(payload);
    if (!message) {
      return false;
    }
    const messageBytes = new TextEncoder().encode(message);

    return await verify(signatureBytes, messageBytes, publicKey);
  } catch (error) {
    return false;
  }
}
