import { describe, test, expect } from 'bun:test';
import { TaskAuction } from '../src/task-auction';
import type { TaskProposal, TaskBid } from '@qualia/types';

function makeProposal(id: string): TaskProposal {
  return {
    id,
    description: `Task ${id}`,
    requiredCapabilities: ['navigate'],
  };
}

function makeBid(taskId: string, agentDid: string, cost: number): TaskBid {
  return { taskId, agentDid, cost, estimatedTime: 1000 };
}

describe('TaskAuction', () => {
  test('propose and bid on a task', () => {
    const auction = new TaskAuction();
    auction.propose(makeProposal('t1'));

    const result = auction.bid(makeBid('t1', 'did:key:zAlice', 10));

    expect(result).toBe(true);
    expect(auction.getBids('t1')).toHaveLength(1);
  });

  test('bid on non-existent task returns false', () => {
    const auction = new TaskAuction();

    const result = auction.bid(makeBid('no-task', 'did:key:zAlice', 10));
    expect(result).toBe(false);
  });

  test('bid on closed auction returns false', () => {
    const auction = new TaskAuction();
    auction.propose(makeProposal('t1'));
    auction.close('t1');

    const result = auction.bid(makeBid('t1', 'did:key:zAlice', 10));
    expect(result).toBe(false);
  });

  test('close returns lowest cost bid', () => {
    const auction = new TaskAuction();
    auction.propose(makeProposal('t1'));
    auction.bid(makeBid('t1', 'did:key:zExpensive', 100));
    auction.bid(makeBid('t1', 'did:key:zCheap', 5));
    auction.bid(makeBid('t1', 'did:key:zMedium', 50));

    const winner = auction.close('t1');

    expect(winner).not.toBeNull();
    expect(winner!.agentDid).toBe('did:key:zCheap');
    expect(winner!.cost).toBe(5);
  });

  test('close returns null for no bids', () => {
    const auction = new TaskAuction();
    auction.propose(makeProposal('t1'));

    const winner = auction.close('t1');
    expect(winner).toBeNull();
  });

  test('close returns null for non-existent auction', () => {
    const auction = new TaskAuction();
    expect(auction.close('no-such')).toBeNull();
  });

  test('getProposals returns only open proposals', () => {
    const auction = new TaskAuction();
    auction.propose(makeProposal('t1'));
    auction.propose(makeProposal('t2'));
    auction.close('t1');

    const proposals = auction.getProposals();
    expect(proposals).toHaveLength(1);
    expect(proposals[0]!.id).toBe('t2');
  });

  test('isOpen reflects auction state', () => {
    const auction = new TaskAuction();
    auction.propose(makeProposal('t1'));

    expect(auction.isOpen('t1')).toBe(true);

    auction.close('t1');
    expect(auction.isOpen('t1')).toBe(false);
  });

  test('isOpen returns false for unknown task', () => {
    const auction = new TaskAuction();
    expect(auction.isOpen('nope')).toBe(false);
  });

  test('cancel removes auction', () => {
    const auction = new TaskAuction();
    auction.propose(makeProposal('t1'));

    expect(auction.cancel('t1')).toBe(true);
    expect(auction.isOpen('t1')).toBe(false);
    expect(auction.getProposals()).toHaveLength(0);
  });

  test('multiple bids tracked correctly', () => {
    const auction = new TaskAuction();
    auction.propose(makeProposal('t1'));
    auction.bid(makeBid('t1', 'did:key:zA', 10));
    auction.bid(makeBid('t1', 'did:key:zB', 20));
    auction.bid(makeBid('t1', 'did:key:zC', 30));

    const bids = auction.getBids('t1');
    expect(bids).toHaveLength(3);
  });
});
