# Hydrasect

Identify and prioritize commits evaluated and built by Hydra when
bisecting Nixpkgs. This can make the Nixpkgs bisection process much
faster, by avoiding unnecessary rebuilds caused by testing commits in
between Hydra evaluations, when commits built by Hydra (with build
results in cache.nixos.org) remain to be tested.

## Installation

🚧 This repo is a [Nix Flake](https://wiki.nixos.org/wiki/Flakes). 🚧

## Usage

First you need to download the latest Hydra evaluations via:

```
$ hydrascrape
Scraping all nixos/unstable-small evaluations from https://hydra.nixos.org...
█████████████████████████████████████████████████████████████████████ 591/591
Replacing old history file with new data.
```

🚧 This currently takes a long time and could be optimized by just
downloading any data. 🚧

The `hydrasect` program, when run in a Nixpkgs bisect, prints the
closest commit(s) to HEAD that have been evaluated by hydra.nixos.org,
and are yet to be tested in the bisect.

So, to run a Hydra-aware Nixpkgs bisect, any time git bisect checks
out a commit to be tested, simply run:

```console
$ git checkout $(hydrasect | head -1)
```

🚧 This will panic if you are not bisecting. 🚧

If there is a suitable Hydra commit, it will be checked out and can be
tested instead of Git's suggestion.

## Acknowledgements

This version of hydrasect is a fork of [this
version](https://git.qyliss.net/hydrasect/) from Alyssa Ross.

### Original License

Copyright 2022 Alyssa Ross <hi@alyssa.is>

Licensed under the EUPL.
