#!/bin/bash

# ===== SETUP

setup() {
  load ../load/base.bash
  load_bats_ext
  setup_home_dir
}

teardown() {
  teardown_home_dir
}

# ===== UTILS

force_kill_node() {
  max_retries=5
  i=0
  while [[ $i -lt $max_retries ]]; do
    pid="$($OCKAM node show $1 --output json | jq .node_pid)"
    run kill -9 $pid
    # Killing a node created without `-f` leaves the
    # process in a defunct state when running within Docker.
    if ! ps -p $pid || ps -p $pid | grep defunct; then
      return
    fi
    sleep 0.2
    ((i = i + 1))
  done
}

# ===== TESTS

@test "node - create with random name" {
  run_success "$OCKAM" node create
}

@test "node - create with name" {
  run_success "$OCKAM" node create n

  run_success "$OCKAM" node show n
  assert_output --partial "/dnsaddr/localhost/tcp/"
  assert_output --partial "/service/api"
  assert_output --partial "/service/uppercase"
}

@test "node - start services" {
  run_success "$OCKAM" node create n1

  # Check we can start service, but only once with the same name
  run_success "$OCKAM" service start hop --addr my_hop --at n1
  run_failure "$OCKAM" service start hop --addr my_hop --at n1
}

@test "node - is restarted with default services" {
  # Create node, check that it has one of the default services running
  run_success "$OCKAM" node create n

  # Stop node, restart it, and check that the service is up again
  $OCKAM node stop n
  run_success "$OCKAM" node start n
  assert_output --partial "/service/echo"
}

@test "node - fail to create two background nodes with the same name" {
  run_success "$OCKAM" node create n
  run_failure "$OCKAM" node create n
}

@test "node - can recreate a background node after it was gracefully stopped" {
  run_success "$OCKAM" node create n
  run_success "$OCKAM" node stop n
  # Recreate node
  run_success "$OCKAM" node create n
}

@test "node - can recreate a background node after it was killed" {
  # This test emulates the situation where a node is killed by the OS
  # on a restart or a shutdown. The node should be able to restart without errors.
  run_success "$OCKAM" node create n

  force_kill_node n

  # Recreate node
  run_success "$OCKAM" node create n
}

@test "node - fail to create node when not existing identity is passed" {
  # Background node
  run_failure "$OCKAM" node create --identity i
  # Foreground node
  run_failure "$OCKAM" node create -f --identity i
}

@test "node - fail to create two foreground nodes with the same name" {
  run_success "$OCKAM" node create n -f &
  sleep 1
  run_success "$OCKAM" node show n
  run_failure "$OCKAM" node create n -f
}

@test "node - can recreate a foreground node after it was killed" {
  run_success "$OCKAM" node create n -f &
  sleep 1
  run_success "$OCKAM" node show n

  force_kill_node n

  # Recreate node
  run_success "$OCKAM" node create n -f &
  sleep 1
  run_success "$OCKAM" node show n
}

@test "node - can recreate a foreground node after it was gracefully stopped" {
  run_success "$OCKAM" node create n -f &
  sleep 1
  run_success "$OCKAM" node show n

  run_success "$OCKAM" node stop n

  # Recreate node
  run_success "$OCKAM" node create n -f &
  sleep 1
  run_success "$OCKAM" node show n
}

@test "node - background node logs to file" {
  run_success "$OCKAM" node create n
  run_success ls -l "$OCKAM_HOME/nodes/n"
  assert_output --partial "stdout"
}

@test "node - foreground node logs to stdout only" {
  run_success "$OCKAM" node create n -vv -f &
  sleep 1
  # It should even create the node directory
  run_failure ls -l "$OCKAM_HOME/nodes/n"
}

@test "node - create a node with an inline configuration" {
  run_success "$OCKAM" node create --node-config "{name: n, tcp-outlets: {db-outlet: {to: 5432, at: n}}}"
  run_success $OCKAM node show n --output json
  assert_output --partial "\"name\": \"n\""
  assert_output --partial "127.0.0.1:5432"
}

@test "node - create two nodes with the same inline configuration" {
  run_success "$OCKAM" node create --node-config "{tcp-outlets: {to: 8080}}"
  run_success "$OCKAM" node create --node-config "{tcp-outlets: {to: 8080}}"

  # each node must have its own outlet
  node_names="$($OCKAM node list --output json | jq -r 'map(.node_name) | join(" ")')"
  for node_name in $node_names; do
    run_success $OCKAM node show $node_name --output json
    assert_output --partial 8080
  done
}

@test "node - return error if passed variable has no value" {
  run_failure "$OCKAM" node create --node-config "{name: n}" --variable MY_VAR=
  assert_output --partial "Empty value for variable 'MY_VAR'"
}
