// The Tauri command and event contract, as TypeScript.
//
// Every type here is generated from the Rust DTOs by ts-rs (`pnpm types:generate`, which runs
// the crates' export tests into ./generated); nothing is mirrored by hand, so a field added or
// renamed in Rust shows up here on the next generation and the type checker finds every place
// that reads it. Field names are camelCase because every Rust DTO serializes that way. The few
// types below the re-export exist only on this side: the event envelope, and the names the
// interface kept when Rust's are longer.

export * from './generated';

import type { AppleCertificateSummary } from './generated/AppleCertificateSummary';
import type { AppleDeviceSummary } from './generated/AppleDeviceSummary';
import type { AppleProvisioningProfileSummary } from './generated/AppleProvisioningProfileSummary';
import type { AppleTeamVerificationResult } from './generated/AppleTeamVerificationResult';
import type { PairingState } from './generated/PairingState';
import type { TransportType } from './generated/TransportType';
import type { TunnelState } from './generated/TunnelState';

/** A machine's progress event: the payload with the machine it belongs to flattened in. */
export type MachineEvent<T> = T & { machineId: string };

/** What the control plane returns for a realtime channel authorization; passed through. */
export interface RealtimeAuthorization {
    auth: string;
    channel_data?: string;
    shared_secret?: string;
}

export type AppleCertificate = AppleCertificateSummary;
export type AppleDevice = AppleDeviceSummary;
export type AppleProvisioningProfile = AppleProvisioningProfileSummary;
export type AppleTeamVerification = AppleTeamVerificationResult;
export type DevicePairingState = PairingState;
export type DeviceTransportType = TransportType;
export type DeviceTunnelState = TunnelState;
