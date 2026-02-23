import type { TaskProposal, TaskBid } from '@qualia/types';

interface Auction {
  proposal: TaskProposal;
  bids: TaskBid[];
  open: boolean;
}

/**
 * Implements a task auction protocol for swarm task allocation.
 * Agents propose tasks and others bid; lowest cost wins.
 */
export class TaskAuction {
  private auctions = new Map<string, Auction>();

  /** Open bidding on a task. */
  propose(proposal: TaskProposal): void {
    this.auctions.set(proposal.id, {
      proposal,
      bids: [],
      open: true,
    });
  }

  /**
   * Submit a bid for a task.
   * Returns false if the task doesn't exist or the auction is closed.
   */
  bid(bid: TaskBid): boolean {
    const auction = this.auctions.get(bid.taskId);
    if (!auction || !auction.open) {
      return false;
    }

    auction.bids.push(bid);
    return true;
  }

  /**
   * Close an auction and return the winning bid (lowest cost).
   * Returns null if no bids or auction doesn't exist.
   */
  close(taskId: string): TaskBid | null {
    const auction = this.auctions.get(taskId);
    if (!auction) return null;

    auction.open = false;

    if (auction.bids.length === 0) return null;

    let winner = auction.bids[0]!;
    for (let i = 1; i < auction.bids.length; i++) {
      const bid = auction.bids[i]!;
      if (bid.cost < winner.cost) {
        winner = bid;
      }
    }

    return winner;
  }

  /** Get all open proposals. */
  getProposals(): TaskProposal[] {
    const result: TaskProposal[] = [];
    for (const auction of this.auctions.values()) {
      if (auction.open) {
        result.push(auction.proposal);
      }
    }
    return result;
  }

  /** Get all bids for a given task. */
  getBids(taskId: string): TaskBid[] {
    const auction = this.auctions.get(taskId);
    if (!auction) return [];
    return [...auction.bids];
  }

  /** Check if an auction is still open. */
  isOpen(taskId: string): boolean {
    const auction = this.auctions.get(taskId);
    if (!auction) return false;
    return auction.open;
  }

  /** Cancel an auction. Returns true if the auction existed. */
  cancel(taskId: string): boolean {
    return this.auctions.delete(taskId);
  }
}
