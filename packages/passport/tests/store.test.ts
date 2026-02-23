/**
 * Tests for passport storage backends
 */

import { describe, expect, it, beforeEach, afterAll } from 'bun:test';
import { MemoryPassportStore, FilePassportStore } from '../src/store';
import { createPassport } from '../src/passport';
import { generateKeypair } from '../src/keypair';
import { unlinkSync, existsSync } from 'fs';
import type { DID } from '@qualia/types';

describe('MemoryPassportStore', () => {
  let store: MemoryPassportStore;

  beforeEach(() => {
    store = new MemoryPassportStore();
  });

  it('should save and retrieve a passport', async () => {
    const keypair = generateKeypair();
    const passport = await createPassport(keypair, ['navigate']);

    await store.save(passport);
    const retrieved = await store.get(passport.did);

    expect(retrieved).toEqual(passport);
  });

  it('should return null for unknown DID', async () => {
    const result = await store.get('did:key:z6MkUnknown' as DID);
    expect(result).toBeNull();
  });

  it('should delete a passport', async () => {
    const keypair = generateKeypair();
    const passport = await createPassport(keypair, []);

    await store.save(passport);
    const deleted = await store.delete(passport.did);
    expect(deleted).toBe(true);

    const retrieved = await store.get(passport.did);
    expect(retrieved).toBeNull();
  });

  it('should return false when deleting non-existent', async () => {
    const deleted = await store.delete('did:key:z6MkNope' as DID);
    expect(deleted).toBe(false);
  });

  it('should list stored DIDs', async () => {
    const kp1 = generateKeypair();
    const kp2 = generateKeypair();
    const p1 = await createPassport(kp1, []);
    const p2 = await createPassport(kp2, []);

    await store.save(p1);
    await store.save(p2);

    const dids = await store.list();
    expect(dids).toContain(p1.did);
    expect(dids).toContain(p2.did);
    expect(dids.length).toBe(2);
  });

  it('should check has()', async () => {
    const keypair = generateKeypair();
    const passport = await createPassport(keypair, []);

    expect(await store.has(passport.did)).toBe(false);
    await store.save(passport);
    expect(await store.has(passport.did)).toBe(true);
  });

  it('should track size', async () => {
    expect(store.size).toBe(0);

    const keypair = generateKeypair();
    const passport = await createPassport(keypair, []);
    await store.save(passport);

    expect(store.size).toBe(1);
  });

  it('should clear all passports', async () => {
    const kp1 = generateKeypair();
    const kp2 = generateKeypair();
    await store.save(await createPassport(kp1, []));
    await store.save(await createPassport(kp2, []));

    store.clear();
    expect(store.size).toBe(0);
    expect((await store.list()).length).toBe(0);
  });
});

describe('FilePassportStore', () => {
  const testFile = '/tmp/qualia-test-passports.json';
  let store: FilePassportStore;

  beforeEach(() => {
    if (existsSync(testFile)) unlinkSync(testFile);
    store = new FilePassportStore(testFile);
  });

  afterAll(() => {
    if (existsSync(testFile)) unlinkSync(testFile);
  });

  it('should save and retrieve a passport', async () => {
    const keypair = generateKeypair();
    const passport = await createPassport(keypair, ['perceive']);

    await store.save(passport);
    const retrieved = await store.get(passport.did);

    expect(retrieved).not.toBeNull();
    expect(retrieved!.did).toBe(passport.did);
    expect(retrieved!.publicKey).toBe(passport.publicKey);
  });

  it('should persist across new instances', async () => {
    const keypair = generateKeypair();
    const passport = await createPassport(keypair, []);

    await store.save(passport);

    // Create new store pointing to same file
    const store2 = new FilePassportStore(testFile);
    const retrieved = await store2.get(passport.did);

    expect(retrieved).not.toBeNull();
    expect(retrieved!.did).toBe(passport.did);
  });

  it('should handle non-existent file gracefully', async () => {
    const missingStore = new FilePassportStore('/tmp/qualia-does-not-exist.json');
    const result = await missingStore.get('did:key:z6MkTest' as DID);
    expect(result).toBeNull();
  });

  it('should list and delete passports', async () => {
    const kp1 = generateKeypair();
    const kp2 = generateKeypair();
    await store.save(await createPassport(kp1, []));
    await store.save(await createPassport(kp2, []));

    const dids = await store.list();
    expect(dids.length).toBe(2);

    await store.delete(dids[0]!);
    expect((await store.list()).length).toBe(1);
  });
});
