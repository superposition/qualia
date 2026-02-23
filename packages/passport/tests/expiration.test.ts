/**
 * Tests for passport expiration
 */

import { describe, expect, it } from 'bun:test';
import {
  createPassport,
  verifyPassport,
  isExpired,
} from '../src/passport';
import { generateKeypair } from '../src/keypair';

describe('Passport Expiration', () => {
  it('should create passport with TTL', async () => {
    const keypair = generateKeypair();
    const passport = await createPassport(keypair, ['navigate'], { ttlSeconds: 3600 });

    expect(passport.expiresAt).toBeDefined();
    expect(passport.expiresAt! - passport.issuedAt).toBe(3600);
  });

  it('should create passport without expiration by default', async () => {
    const keypair = generateKeypair();
    const passport = await createPassport(keypair, []);

    expect(passport.expiresAt).toBeUndefined();
  });

  it('should verify non-expired passport', async () => {
    const keypair = generateKeypair();
    const passport = await createPassport(keypair, [], { ttlSeconds: 3600 });

    const valid = await verifyPassport(passport);
    expect(valid).toBe(true);
  });

  it('should reject expired passport', async () => {
    const keypair = generateKeypair();
    const passport = await createPassport(keypair, [], { ttlSeconds: 1 });

    // Simulate time passing
    const futureTime = passport.issuedAt + 100;
    const valid = await verifyPassport(passport, { currentTime: futureTime });
    expect(valid).toBe(false);
  });

  it('should allow ignoring expiration check', async () => {
    const keypair = generateKeypair();
    const passport = await createPassport(keypair, [], { ttlSeconds: 1 });

    const futureTime = passport.issuedAt + 100;
    const valid = await verifyPassport(passport, {
      currentTime: futureTime,
      ignoreExpiration: true,
    });
    expect(valid).toBe(true);
  });

  it('isExpired returns false for non-expiring passport', async () => {
    const keypair = generateKeypair();
    const passport = await createPassport(keypair, []);

    expect(isExpired(passport)).toBe(false);
  });

  it('isExpired returns false for valid passport', async () => {
    const keypair = generateKeypair();
    const passport = await createPassport(keypair, [], { ttlSeconds: 3600 });

    expect(isExpired(passport)).toBe(false);
  });

  it('isExpired returns true when expired', async () => {
    const keypair = generateKeypair();
    const passport = await createPassport(keypair, [], { ttlSeconds: 1 });

    const futureTime = passport.issuedAt + 100;
    expect(isExpired(passport, futureTime)).toBe(true);
  });
});
