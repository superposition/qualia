# Deployment assets

Checked-in service configuration fragments used to deploy Qualia and the sensor
services it depends on. Each child directory documents one target service and
its installation boundary.

No secret is ever committed here. Service fragments reference an
`EnvironmentFile` under the operator's home directory; the env file itself is
ignored by Git (`**/*.env`, see `.gitignore`).
