#!/usr/bin/env bash
# The licence and attribution guard. NOTICE must survive, and it must keep
# naming exactly the two third-party contributions this product carries.
set -euo pipefail

test -s NOTICE
test -s LICENSE
grep -q "Leash project" NOTICE
grep -q "CC-BY" NOTICE
grep -q "Apache License" LICENSE
echo "NOTICE and LICENSE intact"
