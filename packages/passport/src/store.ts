/**
 * Passport storage interfaces and implementations
 */

import type { Passport, DID } from '@qualia/types';

/**
 * Interface for passport storage backends
 */
export interface PassportStore {
  /** Save a passport, keyed by DID */
  save(passport: Passport): Promise<void>;
  /** Retrieve a passport by DID */
  get(did: DID): Promise<Passport | null>;
  /** Delete a passport by DID */
  delete(did: DID): Promise<boolean>;
  /** List all stored DIDs */
  list(): Promise<DID[]>;
  /** Check if a passport exists for DID */
  has(did: DID): Promise<boolean>;
}

/**
 * In-memory passport store — useful for testing and ephemeral use
 */
export class MemoryPassportStore implements PassportStore {
  private store = new Map<string, Passport>();

  async save(passport: Passport): Promise<void> {
    this.store.set(passport.did, passport);
  }

  async get(did: DID): Promise<Passport | null> {
    return this.store.get(did) ?? null;
  }

  async delete(did: DID): Promise<boolean> {
    return this.store.delete(did);
  }

  async list(): Promise<DID[]> {
    return Array.from(this.store.keys()) as DID[];
  }

  async has(did: DID): Promise<boolean> {
    return this.store.has(did);
  }

  /** Number of stored passports */
  get size(): number {
    return this.store.size;
  }

  /** Clear all stored passports */
  clear(): void {
    this.store.clear();
  }
}

/**
 * File-based passport store — persists passports to a JSON file
 */
export class FilePassportStore implements PassportStore {
  private filePath: string;
  private cache: Map<string, Passport> | null = null;

  constructor(filePath: string) {
    this.filePath = filePath;
  }

  private async load(): Promise<Map<string, Passport>> {
    if (this.cache) return this.cache;

    try {
      const file = Bun.file(this.filePath);
      if (await file.exists()) {
        const text = await file.text();
        const data = JSON.parse(text) as Record<string, Passport>;
        this.cache = new Map(Object.entries(data));
      } else {
        this.cache = new Map();
      }
    } catch {
      this.cache = new Map();
    }
    return this.cache;
  }

  private async persist(): Promise<void> {
    const store = await this.load();
    const data: Record<string, Passport> = {};
    for (const [key, value] of store) {
      data[key] = value;
    }
    await Bun.write(this.filePath, JSON.stringify(data, null, 2));
  }

  async save(passport: Passport): Promise<void> {
    const store = await this.load();
    store.set(passport.did, passport);
    await this.persist();
  }

  async get(did: DID): Promise<Passport | null> {
    const store = await this.load();
    return store.get(did) ?? null;
  }

  async delete(did: DID): Promise<boolean> {
    const store = await this.load();
    const existed = store.delete(did);
    if (existed) await this.persist();
    return existed;
  }

  async list(): Promise<DID[]> {
    const store = await this.load();
    return Array.from(store.keys()) as DID[];
  }

  async has(did: DID): Promise<boolean> {
    const store = await this.load();
    return store.has(did);
  }
}
