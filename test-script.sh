#!/bin/bash
# Test script - prints to stdout and stderr alternating, every 10 seconds

count=0
while true; do
    count=$((count + 1))
    timestamp=$(date +"%Y-%m-%d %H:%M:%S.%N")
    echo "[STDOUT $count] $timestamp"
    echo "[STDERR $count] $timestamp" >&2
    sleep 10
done
