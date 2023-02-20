# The zkevm-node docker-compose file currently only works if run from the zkevm-node/test directory.
compose-base := "docker compose -f docker-compose.yaml -f permissionless-docker-compose.yaml"
compose-anvil := compose-base + " -f docker-compose-anvil.yaml --env-file .env.anvil"
compose := compose-base + " -f docker-compose-geth.yaml"
compose-zkevm-node := "docker compose -f permissionless-docker-compose.yaml -f docker-compose-geth.yaml"

zkevm-node:
    {{compose-zkevm-node}} up -V --force-recreate --abort-on-container-exit

zkevm-node-background:
    {{compose-zkevm-node}} up -d -V --force-recreate

demo:
    {{compose}} up -V --force-recreate --abort-on-container-exit

hermez:
	{{compose}} up -V --force-recreate --abort-on-container-exit -- zkevm-mock-l1-network zkevm-state-db zkevm-permissionless-node zkevm-aggregator zkevm-prover

# This currently doesn't quite work yet, likely because the block number is zero
# after anvil loads state.
demo-anvil:
    {{compose-anvil}} up -V --force-recreate --abort-on-container-exit

demo-background:
    {{compose}} up -d -V --force-recreate

down:
    {{compose}} down -v --remove-orphans

pull:
    {{compose}} pull

hardhat *args:
    cd zkevm-contracts && nix develop -c bash -c "npx hardhat {{args}}"

npm *args:
   cd zkevm-contracts && nix develop -c bash -c "npm {{args}}"

compose *args:
   {{compose}} {{args}}

docker-stop-rm:
    docker stop $(docker ps -aq); docker rm $(docker ps -aq)

anvil *args:
    docker run ghcr.io/foundry-rs/foundry:latest "anvil {{args}}"
