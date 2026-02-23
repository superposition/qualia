import type { AreaClaim } from '@qualia/types';

/**
 * Manages area claims for spatial deconfliction.
 * Uses circle-circle intersection to detect overlaps.
 */
export class AreaManager {
  private claims = new Map<string, AreaClaim>();

  /**
   * Claim an area. Returns false if the area overlaps with an existing unexpired claim.
   */
  claim(claim: AreaClaim): boolean {
    // Prune expired claims before checking
    this.pruneExpired();

    // Check for overlaps with existing active claims
    for (const existing of this.claims.values()) {
      if (this.overlaps(existing, claim)) {
        return false;
      }
    }

    this.claims.set(claim.id, claim);
    return true;
  }

  /** Release a claim by ID. Returns true if the claim existed. */
  release(claimId: string): boolean {
    return this.claims.delete(claimId);
  }

  /** Get all active (unexpired) claims. */
  getClaims(): AreaClaim[] {
    const now = Date.now();
    return [...this.claims.values()].filter((c) => c.expiresAt > now);
  }

  /** Find the claim covering a given point, or null if none. */
  getClaimAt(x: number, y: number): AreaClaim | null {
    const now = Date.now();

    for (const claim of this.claims.values()) {
      if (claim.expiresAt <= now) continue;
      const dist = Math.hypot(claim.center.x - x, claim.center.y - y);
      if (dist <= claim.radius) {
        return claim;
      }
    }

    return null;
  }

  /** Check if an area (circle) is unclaimed. */
  isAvailable(center: { x: number; y: number }, radius: number): boolean {
    const now = Date.now();

    for (const claim of this.claims.values()) {
      if (claim.expiresAt <= now) continue;
      if (this.circlesOverlap(claim.center, claim.radius, center, radius)) {
        return false;
      }
    }

    return true;
  }

  /**
   * Remove expired claims.
   * @returns Number of claims removed.
   */
  pruneExpired(): number {
    const now = Date.now();
    let removed = 0;

    for (const [id, claim] of this.claims) {
      if (claim.expiresAt <= now) {
        this.claims.delete(id);
        removed++;
      }
    }

    return removed;
  }

  /** Circle-circle intersection: distance between centers < sum of radii. */
  private overlaps(a: AreaClaim, b: AreaClaim): boolean {
    return this.circlesOverlap(a.center, a.radius, b.center, b.radius);
  }

  private circlesOverlap(
    c1: { x: number; y: number },
    r1: number,
    c2: { x: number; y: number },
    r2: number,
  ): boolean {
    const dist = Math.hypot(c1.x - c2.x, c1.y - c2.y);
    return dist < r1 + r2;
  }
}
