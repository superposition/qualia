/**
 * Passport creation and verification
 */

import canonicalize from 'canonicalize';
import type { Passport, KeyPair } from '@qualia/types';
import { publicKeyToDID, didToPublicKey, isValidDID } from './did';
import { sign, verify } from './signature';
import { toHex, fromHex } from './utils';

/**
 * Create a passport for an agent identity
 * @param keypair - Ed25519 keypair
 * @param capabilities - Agent capabilities
 * @returns Signed passport
 */
export async function createPassport(
  keypair: KeyPair,
  capabilities: string[] = []
): Promise<Passport> {
  const did = publicKeyToDID(keypair.publicKey);

  const issuedAt = Math.floor(Date.now() / 1000);
  const publicKeyHex = toHex(keypair.publicKey);

  const passportPayload = {
    did,
    publicKey: publicKeyHex,
    capabilities,
    issuedAt,
  };

  // Sign the passport payload using canonical JSON (RFC 8785)
  const payloadString = canonicalize(passportPayload);
  if (!payloadString) {
    throw new Error('Failed to canonicalize passport payload');
  }
  const payloadBytes = new TextEncoder().encode(payloadString);
  const signatureBytes = await sign(payloadBytes, keypair.privateKey);
  const signature = toHex(signatureBytes);

  const passport: Passport = {
    ...passportPayload,
    signature,
  };

  return passport;
}

/**
 * Verify a passport signature
 * @param passport - Passport to verify
 * @returns True if passport is valid
 */
export async function verifyPassport(passport: Passport): Promise<boolean> {
  try {
    if (!isValidDID(passport.did)) {
      return false;
    }

    if (
      !passport.publicKey ||
      typeof passport.publicKey !== 'string' ||
      !/^[0-9a-f]{64}$/.test(passport.publicKey)
    ) {
      return false;
    }

    if (
      !passport.signature ||
      typeof passport.signature !== 'string' ||
      !/^[0-9a-f]{128}$/.test(passport.signature)
    ) {
      return false;
    }

    // Extract public key from DID
    const publicKeyFromDID = didToPublicKey(passport.did);

    // Verify that public key in passport matches DID
    const publicKeyFromPassport = fromHex(passport.publicKey);

    if (
      publicKeyFromDID.length !== publicKeyFromPassport.length ||
      !publicKeyFromDID.every((byte, i) => byte === publicKeyFromPassport[i])
    ) {
      return false;
    }

    // Reconstruct the signed payload (without signature)
    const passportPayload = {
      did: passport.did,
      publicKey: passport.publicKey,
      capabilities: passport.capabilities,
      issuedAt: passport.issuedAt,
    };

    const payloadString = canonicalize(passportPayload);
    if (!payloadString) {
      return false;
    }
    const payloadBytes = new TextEncoder().encode(payloadString);
    const signatureBytes = fromHex(passport.signature);

    return await verify(signatureBytes, payloadBytes, publicKeyFromDID);
  } catch (error) {
    return false;
  }
}

/**
 * Serialize passport to JSON string
 * @param passport - Passport to serialize
 * @returns JSON string
 */
export function serializePassport(passport: Passport): string {
  return JSON.stringify(passport);
}

/**
 * Deserialize passport from JSON string
 * @param serialized - JSON string
 * @returns Passport object
 * @throws Error if JSON is invalid or missing required fields
 */
export function deserializePassport(serialized: string): Passport {
  try {
    const parsed = JSON.parse(serialized);

    if (
      !parsed.did ||
      !parsed.publicKey ||
      !parsed.signature ||
      typeof parsed.issuedAt !== 'number' ||
      !Array.isArray(parsed.capabilities)
    ) {
      throw new Error('Missing required passport fields');
    }

    const passport: Passport = {
      did: parsed.did,
      publicKey: parsed.publicKey,
      capabilities: parsed.capabilities,
      signature: parsed.signature,
      issuedAt: parsed.issuedAt,
    };

    return passport;
  } catch (error) {
    throw new Error(
      `Failed to deserialize passport: ${error instanceof Error ? error.message : 'Unknown error'}`
    );
  }
}
