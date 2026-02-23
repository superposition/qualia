# Contributing to QUALIA

Thanks for your interest in contributing!

## Getting Started

1. Fork the repo
2. Clone your fork: `git clone https://github.com/YOUR_USERNAME/qualia.git`
3. Install dependencies: `bun install`
4. Create a branch: `git checkout -b feature/your-feature`
5. Make your changes
6. Run checks: `bun run typecheck && bun run test`
7. Push and open a PR

## Development Setup

- **Runtime**: [Bun](https://bun.sh/) >= 1.1.0
- **Language**: TypeScript 5.5+
- **Build**: [Turborepo](https://turbo.build/)

## Project Structure

```
packages/        # Shared libraries (types, a2a, passport, ros-client, etc.)
apps/            # Applications (hardware-bridge, etc.)
examples/        # Example projects
```

## Guidelines

- Write tests for new functionality
- Keep PRs focused — one feature or fix per PR
- Follow existing code style (run `bun run format` before committing)
- Update relevant documentation

## Reporting Issues

Open an issue on GitHub with:
- What you expected to happen
- What actually happened
- Steps to reproduce
- Environment details (OS, Bun version, robot platform if relevant)
