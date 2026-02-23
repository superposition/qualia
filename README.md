# QUALIA

Open robotics framework — A2A protocol, DID identity, ROS integration for robots and AI agents.

**TypeScript/Bun monorepo** for building networked robot systems with decentralized identity and inter-agent communication.

## Packages

| Package | Description |
|---|---|
| `@qualia/types` | Shared type definitions (NANDA, DID, A2A, ROS, capabilities) |
| `@qualia/passport` | Ed25519 identity — DID generation, passport signing, verification |
| `@qualia/a2a` | Agent-to-Agent JSON-RPC protocol with DID authentication |
| `@qualia/ros-client` | ROS Bridge WebSocket client (topics, services, auto-reconnect) |
| `@qualia/registry` | NANDA agent registry — on-chain registration and discovery |
| `@qualia/monitor` | Robot monitoring — SSH, network scanning, ROS2 status, telemetry |

## Apps

| App | Description |
|---|---|
| `@qualia/hardware-bridge` | Generic robot hardware bridge — translates ROS commands to serial/GPIO |
| `robot-cli` | Python CLI for robot management |

## Quick Start

```bash
# Install dependencies
bun install

# Build all packages
bun run build

# Run tests
bun run test

# Typecheck
bun run typecheck
```

## Architecture

```mermaid
graph TB
    subgraph Apps["Applications"]
        HB[hardware-bridge]
        CLI[robot-cli]
        YOUR[your-app]
    end

    subgraph Protocols
        A2A["@qualia/a2a<br/>Agent-to-Agent JSON-RPC"]
        ROS["@qualia/ros-client<br/>ROS Bridge WebSocket"]
    end

    subgraph Identity
        PASS["@qualia/passport<br/>Ed25519 DID & Passports"]
        REG["@qualia/registry<br/>NANDA Agent Registry"]
    end

    subgraph Infrastructure
        MON["@qualia/monitor<br/>Robot Monitoring & Telemetry"]
        TYPES["@qualia/types<br/>Shared Type Definitions"]
    end

    HB --> ROS
    HB --> A2A
    CLI --> MON
    YOUR --> A2A
    YOUR --> ROS

    A2A --> PASS
    A2A --> TYPES
    ROS --> TYPES
    REG --> PASS
    REG --> TYPES
    MON --> TYPES
    PASS --> TYPES
```

## A2A Protocol Flow

```mermaid
sequenceDiagram
    participant R1 as Robot A
    participant REG as NANDA Registry
    participant R2 as Robot B

    R1->>REG: Register DID + capabilities
    R2->>REG: Register DID + capabilities

    R1->>REG: Discover agents with "navigate" capability
    REG-->>R1: Robot B (DID, endpoint)

    R1->>R2: JSON-RPC request (signed with Ed25519)
    R2->>R2: Verify DID signature
    R2-->>R1: JSON-RPC response

    Note over R1,R2: Authenticated agent-to-agent communication
```

## Identity System

```mermaid
graph LR
    KP[Ed25519 Keypair] --> DID["did:key:z6Mk..."]
    DID --> PASSPORT[Signed Passport]
    PASSPORT --> IPFS[IPFS Storage]
    PASSPORT --> CHAIN[On-chain Registry]
    DID --> A2A_AUTH[A2A Request Signing]
    DID --> ROS_AUTH[Robot Authentication]
```

## ROS Integration

```mermaid
graph LR
    subgraph Robot
        HW[Hardware<br/>Motors / Sensors]
        BRIDGE[Hardware Bridge<br/>Serial / GPIO]
        ROSB[rosbridge_suite<br/>WebSocket Server]
    end

    subgraph Network
        CLIENT["@qualia/ros-client<br/>WebSocket Client"]
        AGENT[AI Agent / App]
    end

    AGENT --> CLIENT
    CLIENT -->|"ws://robot:9090"| ROSB
    ROSB --> BRIDGE
    BRIDGE -->|serial / GPIO| HW
    HW -->|sensor data| BRIDGE
    BRIDGE --> ROSB
    ROSB --> CLIENT
    CLIENT --> AGENT
```

## Supported Robots

The hardware bridge uses a driver/plugin system. Write a driver for any robot platform:

- Differential drive (e.g., Waveshare UGV)
- Ackermann steering
- Mecanum/omnidirectional wheels
- Custom serial/GPIO/I2C/CAN protocols

## Development

```bash
# Start a specific package in dev mode
bun run dev --filter=@qualia/ros-client

# Run tests for a specific package
bun run test --filter=@qualia/a2a

# Format code
bun run format
```

## License

MIT
