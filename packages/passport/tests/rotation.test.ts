/**
 * Tests for key rotation and batch verification
 */

import { describe, expect, it } from 'bun:test';
import {
  createPassport,
  verifyPassport,
  batchVerify,
  createKeyRotationProof,
  verifyKeyRotationProof,
  rotatePassport,
} from '../src/passport';
import { generateKeypair } from '../src/keypair';
import { publicKeyToDID } from '../src/did';

describe('Key Rotation', () => {
  it('should create a valid rotation proof', async () => {
    const oldKeypair = generateKeypair();
    const newKeypair = generateKeypair();

    const proof = await createKeyRotationProof(oldKeypair, newKeypair);

    expect(proof.oldDid).toBe(publicKeyToDID(oldKeypair.publicKey));
    expect(proof.newDid).toBe(publicKeyToDID(newKeypair.publicKey));
    expect(proof.signature).toMatch(/^[0-9a-f]{128}$/);
    expect(proof.timestamp).toBeGreaterThan(0);
  });

  it('should verify valid rotation proof', async () => {
    const oldKeypair = generateKeypair();
    const newKeypair = generateKeypair();

    const proof = await createKeyRotationProof(oldKeypair, newKeypair);
    const valid = await verifyKeyRotationProof(proof);

    expect(valid).toBe(true);
  });

  it('should reject tampered rotation proof', async () => {
    const oldKeypair = generateKeypair();
    const newKeypair = generateKeypair();

    const proof = await createKeyRotationProof(oldKeypair, newKeypair);
    proof.newDid = publicKeyToDID(generateKeypair().publicKey);

    const valid = await verifyKeyRotationProof(proof);
    expect(valid).toBe(false);
  });

  it('should reject rotation proof signed by wrong key', async () => {
    const oldKeypair = generateKeypair();
    const newKeypair = generateKeypair();
    const wrongKeypair = generateKeypair();

    // Create proof signed by wrong key
    const proof = await createKeyRotationProof(wrongKeypair, newKeypair);
    // But claim it's from oldKeypair
    proof.oldDid = publicKeyToDID(oldKeypair.publicKey);

    const valid = await verifyKeyRotationProof(proof);
    expect(valid).toBe(false);
  });

  it('should rotate passport to new keypair', async () => {
    const oldKeypair = generateKeypair();
    const newKeypair = generateKeypair();
    const capabilities = ['navigate', 'perceive'];

    const oldPassport = await createPassport(oldKeypair, capabilities);
    const { passport: newPassport, proof } = await rotatePassport(
      oldPassport,
      oldKeypair,
      newKeypair,
    );

    // New passport should be valid
    const newValid = await verifyPassport(newPassport);
    expect(newValid).toBe(true);

    // New passport should have new DID
    expect(newPassport.did).toBe(publicKeyToDID(newKeypair.publicKey));
    expect(newPassport.did).not.toBe(oldPassport.did);

    // Capabilities preserved
    expect(newPassport.capabilities).toEqual(capabilities);

    // Rotation proof should be valid
    const proofValid = await verifyKeyRotationProof(proof);
    expect(proofValid).toBe(true);
  });

  it('should rotate passport with TTL', async () => {
    const oldKeypair = generateKeypair();
    const newKeypair = generateKeypair();

    const oldPassport = await createPassport(oldKeypair, []);
    const { passport } = await rotatePassport(
      oldPassport,
      oldKeypair,
      newKeypair,
      { ttlSeconds: 7200 },
    );

    expect(passport.expiresAt).toBeDefined();
    expect(passport.expiresAt! - passport.issuedAt).toBe(7200);
  });
});

describe('Batch Verification', () => {
  it('should verify multiple valid passports', async () => {
    const passports = await Promise.all(
      Array.from({ length: 5 }, async () => {
        const kp = generateKeypair();
        return createPassport(kp, ['navigate']);
      }),
    );

    const results = await batchVerify(passports);

    expect(results.length).toBe(5);
    expect(results.every(r => r.valid)).toBe(true);
  });

  it('should identify invalid passports in batch', async () => {
    const kp1 = generateKeypair();
    const kp2 = generateKeypair();

    const valid = await createPassport(kp1, []);
    const invalid = await createPassport(kp2, []);
    invalid.signature = 'a'.repeat(128); // Tamper

    const results = await batchVerify([valid, invalid]);

    expect(results[0]!.valid).toBe(true);
    expect(results[1]!.valid).toBe(false);
  });

  it('should handle empty batch', async () => {
    const results = await batchVerify([]);
    expect(results).toEqual([]);
  });

  it('should respect expiration in batch verify', async () => {
    const kp1 = generateKeypair();
    const kp2 = generateKeypair();

    const nonExpiring = await createPassport(kp1, []);
    const expiring = await createPassport(kp2, [], { ttlSeconds: 1 });

    const futureTime = expiring.issuedAt + 100;
    const results = await batchVerify([nonExpiring, expiring], { currentTime: futureTime });

    expect(results[0]!.valid).toBe(true);
    expect(results[1]!.valid).toBe(false);
  });
});
