#!/bin/bash

anchor --provider.cluster mainnet \
  idl fetch 6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P \
  -o idl/pumpfun.json

anchor --provider.cluster mainnet \
  idl fetch pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA \
  -o idl/pumpswap.json