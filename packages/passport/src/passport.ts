/**
 * Passport creation and verification
 */

import canonicalize from 'canonicalize';
import type { Passport, KeyPair } from '@qualia/types';
import { publicKeyToDID, didToPublicKey, isValidDID } from './did';
import { sign, verify } from './signature';
import { toHex, fromHex } from './utils';

/** Options for passport creation */
export interface CreatePassportOptions {
  /** Time-to-live in seconds (sets expiresAt = issuedAt + ttlSeconds) */
  ttlSeconds?: number;
}

/**
 * Create a passport for an agent identity
 * @param keypair - Ed25519 keypair
 * @param capabilities - Agent capabilities
 * @param options - Optional creation options (e.g., TTL)
 * @returns Signed passport
 */
export async function createPassport(
  keypair: KeyPair,
  capabilities: string[] = [],
  options?: CreatePassportOptions,
): Promise<Passport> {
  // Derive DID from public key
  const did = publicKeyToDID(keypair.publicKey);

  // Create passport payload
  const issuedAt = Math.floor(Date.now() / 1000);
  const publicKeyHex = toHex(keypair.publicKey);

  // Build payload with optional expiration
  const passportPayload: Record<string, unknown> = {
    did,
    publicKey: publicKeyHex,
    capabilities,
    issuedAt,
  };

  if (options?.ttlSeconds !== undefined) {
    passportPayload['expiresAt'] = issuedAt + options.ttlSeconds;
  }

  // Sign the passport payload using canonical JSON (RFC 8785)
  const payloadString = canonicalize(passportPayload);
  if (!payloadString) {
    throw new Error('Failed to canonicalize passport payload');
  }
  const payloadBytes = new TextEncoder().encode(payloadString);
  const signatureBytes = await sign(payloadBytes, keypair.privateKey);
  const signature = toHex(signatureBytes);

  // Return complete passport
  const passport: Passport = {
    did,
    publicKey: publicKeyHex,
    capabilities,
    issuedAt,
    signature,
  };

  if (options?.ttlSeconds !== undefined) {
    passport.expiresAt = issuedAt + options.ttlSeconds;
  }

  return passport;
}

/** Options for passport verification */
export interface VerifyPassportOptions {
  /** If true, skip expiration check (default: false) */
  ignoreExpiration?: boolean;
  /** Custom current time for expiration check (Unix seconds) */
  currentTime?: number;
}

/**
 * Verify a passport signature and optionally check expiration
 * @param passport - Passport to verify
 * @param options - Optional verification options
 * @returns True if passport is valid
 */
export async function verifyPassport(
  passport: Passport,
  options?: VerifyPassportOptions,
): Promise<boolean> {
  try {
    // Validate DID format
    if (!isValidDID(passport.did)) {
      return false;
    }

    // Validate public key hex format
    if (
      !passport.publicKey ||
      typeof passport.publicKey !== 'string' ||
      !/^[0-9a-f]{64}$/.test(passport.publicKey)
    ) {
      return false;
    }

    // Validate signature format
    if (
      !passport.signature ||
      typeof passport.signature !== 'string' ||
      !/^[0-9a-f]{128}$/.test(passport.signature)
    ) {
      return false;
    }

    // Check expiration
    if (!options?.ignoreExpiration && passport.expiresAt !== undefined) {
      const now = options?.currentTime ?? Math.floor(Date.now() / 1000);
      if (now >= passport.expiresAt) {
        return false;
      }
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
    const passportPayload: Record<string, unknown> = {
      did: passport.did,
      publicKey: passport.publicKey,
      capabilities: passport.capabilities,
      issuedAt: passport.issuedAt,
    };

    if (passport.expiresAt !== undefined) {
      passportPayload['expiresAt'] = passport.expiresAt;
    }

    // Serialize using canonical JSON (RFC 8785)
    const payloadString = canonicalize(passportPayload);
    if (!payloadString) {
      return false;
    }
    const payloadBytes = new TextEncoder().encode(payloadString);
    const signatureBytes = fromHex(passport.signature);

    // Verify signature
    return await verify(signatureBytes, payloadBytes, publicKeyFromDID);
  } catch {
    // Any error during verification means invalid passport
    return false;
  }
}

/**
 * Verify multiple passports in batch (for fleet scenarios)
 * @param passports - Array of passports to verify
 * @param options - Optional verification options
 * @returns Array of results with DID and validity
 */
export async function batchVerify(
  passports: Passport[],
  options?: VerifyPassportOptions,
): Promise<Array<{ did: string; valid: boolean }>> {
  const results = await Promise.all(
    passports.map(async (passport) => ({
      did: passport.did,
      valid: await verifyPassport(passport, options),
    })),
  );
  return results;
}

/**
 * Check if a passport is expired
 * @param passport - Passport to check
 * @param currentTime - Optional custom current time (Unix seconds)
 * @returns True if the passport has an expiresAt field and it has passed
 */
export function isExpired(passport: Passport, currentTime?: number): boolean {
  if (passport.expiresAt === undefined) return false;
  const now = currentTime ?? Math.floor(Date.now() / 1000);
  return now >= passport.expiresAt;
}

/** Key rotation proof — signed by old key, authorizes new key */
export interface KeyRotationProof {
  /** DID of the old key */
  oldDid: string;
  /** DID of the new key */
  newDid: string;
  /** Hex-encoded new public key */
  newPublicKey: string;
  /** Signature by old private key over canonical JSON of {oldDid, newDid, newPublicKey, timestamp} */
  signature: string;
  /** Unix timestamp in seconds */
  timestamp: number;
}

/**
 * Create a key rotation proof — signed by old key, authorizing new key
 * @param oldKeypair - Current keypair being rotated away from
 * @param newKeypair - New keypair being rotated to
 * @returns Signed key rotation proof
 */
export async function createKeyRotationProof(
  oldKeypair: KeyPair,
  newKeypair: KeyPair,
): Promise<KeyRotationProof> {
  const oldDid = publicKeyToDID(oldKeypair.publicKey);
  const newDid = publicKeyToDID(newKeypair.publicKey);
  const newPublicKey = toHex(newKeypair.publicKey);
  const timestamp = Math.floor(Date.now() / 1000);

  const payload = { oldDid, newDid, newPublicKey, timestamp };
  const payloadString = canonicalize(payload);
  if (!payloadString) {
    throw new Error('Failed to canonicalize rotation proof payload');
  }

  const payloadBytes = new TextEncoder().encode(payloadString);
  const signatureBytes = await sign(payloadBytes, oldKeypair.privateKey);

  return {
    oldDid,
    newDid,
    newPublicKey,
    signature: toHex(signatureBytes),
    timestamp,
  };
}

/**
 * Verify a key rotation proof
 * @param proof - The rotation proof to verify
 * @returns True if the proof is valid (signed by old key)
 */
export async function verifyKeyRotationProof(
  proof: KeyRotationProof,
): Promise<boolean> {
  try {
    if (!isValidDID(proof.oldDid as any)) return false;
    if (!isValidDID(proof.newDid as any)) return false;

    const oldPublicKey = didToPublicKey(proof.oldDid as any);
    const payload = {
      oldDid: proof.oldDid,
      newDid: proof.newDid,
      newPublicKey: proof.newPublicKey,
      timestamp: proof.timestamp,
    };

    const payloadString = canonicalize(payload);
    if (!payloadString) return false;

    const payloadBytes = new TextEncoder().encode(payloadString);
    const signatureBytes = fromHex(proof.signature);

    return await verify(signatureBytes, payloadBytes, oldPublicKey);
  } catch {
    return false;
  }
}

/**
 * Rotate a passport to a new keypair — creates new passport signed by new key
 * with proof chain back to old key
 * @param oldPassport - The existing passport
 * @param oldKeypair - The current keypair
 * @param newKeypair - The new keypair to rotate to
 * @param options - Optional creation options
 * @returns Object with new passport and rotation proof
 */
export async function rotatePassport(
  oldPassport: Passport,
  oldKeypair: KeyPair,
  newKeypair: KeyPair,
  options?: CreatePassportOptions,
): Promise<{ passport: Passport; proof: KeyRotationProof }> {
  const proof = await createKeyRotationProof(oldKeypair, newKeypair);
  const passport = await createPassport(
    newKeypair,
    oldPassport.capabilities,
    options,
  );
  return { passport, proof };
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

    // Validate required fields
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
