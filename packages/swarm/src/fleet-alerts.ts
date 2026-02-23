import type { FleetAlert, AlertType, SwarmMessage } from '@qualia/types';

/**
 * Manages fleet-wide alerts for swarm communication.
 */
export class FleetAlertManager {
  private alerts: SwarmMessage<FleetAlert>[] = [];

  /** Create and store an alert message. */
  raise(alert: FleetAlert, fromDid: string): SwarmMessage<FleetAlert> {
    const message: SwarmMessage<FleetAlert> = {
      type: 'alert',
      from: fromDid,
      data: alert,
      timestamp: Date.now(),
    };

    this.alerts.push(message);
    return message;
  }

  /**
   * Get recent alerts, optionally filtered by max age.
   * @param maxAgeMs - Maximum age in milliseconds (default: all)
   */
  getRecent(maxAgeMs?: number): SwarmMessage<FleetAlert>[] {
    if (maxAgeMs === undefined) {
      return [...this.alerts];
    }

    const cutoff = Date.now() - maxAgeMs;
    return this.alerts.filter((a) => a.timestamp >= cutoff);
  }

  /** Get alerts filtered by type. */
  getByType(type: AlertType): SwarmMessage<FleetAlert>[] {
    return this.alerts.filter((a) => a.data.type === type);
  }

  /** Clear all stored alerts. */
  clear(): void {
    this.alerts = [];
  }
}
