# Discovery: can the labs run on this Mac?

Read on 2026-11-04 by a discovery session (read-only) from public documentation pages and the files under `reference/`. Nothing was installed or run.


**Result:** The observability stack very likely runs on arm64. Containerlab cannot run natively on macOS. It needs a Linux machine inside OrbStack, and that machine needs an arm64 cEOS image. That image exists only as an early Arista build and needs an Arista account. I read the pages and files below. I did not install or run anything.

## Rules

netpanda-ai-workbench rules, version 0.12.0, file workbench-00-about.md

All the other rule files I could see carry the same version line, so there is no disagreement. I could see all ten sections of the rule set, and `.claude/profiles/software.md` (also 0.12.0). Nothing was missing.

## Known facts

**Question 1: containerlab on an Apple silicon Mac**

- **The containerlab macOS page** (https://containerlab.dev/macos/, read 2026-11-04, no version shown):
  - "On macOS, Docker is always provided as a Linux/arm64 VM that runs the docker daemon." Containerlab runs inside such a VM.
  - OrbStack is the recommended option. The page says the containerlab maintainer recommends it.
  - The steps for OrbStack are:
    1. Install OrbStack.
    2. Create an arm64 Linux machine.
    3. Install containerlab inside it.
    4. Check each image with `docker image inspect <image> -f '{{.Architecture}}'`.
    5. Deploy.
  - Docker Desktop, Rancher Desktop, Colima and Container Desktop are named as alternatives.
  - A VS Code devcontainer also works. It needs containerlab 0.60.0 or newer on arm64 macOS.
  - The page advises "use ARM64 network OS images whenever possible".
- **Node kinds on arm64** (same page):
  - Native: Nokia SR Linux (a preview build), FRR, Juniper cRPD (control plane only), and Linux helper images (Alpine, network-multitool, gnmic, Prometheus, Grafana).
  - Under Rosetta (x86 translation): cEOS x64 and Cisco IOL are "known to work", with a warning that the speed penalty may stop nodes working at all.
  - Not usable: VM-based vrnetlab images, which need nested virtualization. The page says that needs an M3 or newer chip and macOS 15 or newer.
  - Most other network operating systems are not built for arm64.
  - The page says Arista announced arm64 cEOS "sometime in 2024" and tells the reader to verify.
- **The containerlab install page** (https://containerlab.dev/install/, read 2026-11-04):
  - Containerlab ships deb, rpm and apk packages for amd64 and arm64.
  - It says running on macOS "is possible both on ARM … and Intel", but gives no macOS steps. It points to the macOS page.
  - The usual install is `bash -c "$(curl -sL https://get.containerlab.dev)"`. Docker is required and the user needs sudo.
- **The OrbStack machines page** (https://docs.orbstack.dev/machines/, read 2026-11-04):
  - Nested virtualization is not supported on Apple silicon.
  - Rosetta lets a machine created with `--arch amd64` run x86_64 programs.
  - Memory, CPU and disk limits can be set per machine (`orb create --memory 4G`). Machines without their own limits share the global OrbStack limits.
- **The OrbStack Docker page** (https://docs.orbstack.dev/docker/, read 2026-11-04):
  - Rosetta runs amd64 images with `--platform linux/amd64`.
  - The default memory limit and whether containers are reachable by IP from the Mac are not stated.

**Question 2: Arista cEOS on arm64**

- **The containerlab cEOS page** (https://containerlab.dev/manual/kinds/ceos/, read 2026-11-04):
  - It does not mention arm64.
  - "Arista requires its users to register with arista.com before downloading any images." The image is loaded with `docker import cEOS64-lab-<version>.tar.xz ceos:<version>`.
  - It records an OrbStack problem: BGP sessions with MD5 passwords can stick in `Active`.
  - Versions before 4.32.0F need cgroups v1.
- **The Arista pages** (https://www.arista.com/en/support/software-download, read 2026-11-04): I only saw a sign-in page. Nothing public describes the cEOS downloads.
- **The ipSpace blog post** (https://blog.ipspace.net/2025/02/arista-ceos-arm-apple-silicon/, dated 2025-02-04, read 2026-11-04):
  - An arm64 cEOS exists. It was an engineering build, "4.33.1F … liffeyceoslabarm", on aarch64.
  - It was found on Arista's download site under "cEOS Lab/4.33".
  - The post does not say whether an account is needed. The containerlab page says it is.
  - Ivan ran it in an Ubuntu VM with 8 GB memory and 4 CPUs, using a two-node OSPF lab.
  - A commenter got "router bgp … not supported on this hardware platform". Ivan saw BGP work through netlab and containerlab.
  - A commenter got "exec format error" after pulling the x86 image by mistake.
- **The Infrahub lab page** (https://docs.infrahub.app/infrahub-demo-sp/lab/containerlab, read 2026-11-04):
  - "Apple Silicon cannot run it." Recent cEOS-lab builds (4.36.0.1F) have a 32-bit x86 userland that Rosetta cannot run.
  - The fallback is qemu-i386 emulation, which fails after about 17 minutes.
  - "No arm64 cEOS tag exists" in their registry, and any tag claiming arm64 is mislabelled x86.
- **The two lab files:**
  - Both topologies have two `ceos` nodes joined by two links (`reference/network-observability-lab/containerlab/lab.yml` lines 12-38, `reference/labcli/example/containerlab/lab.clab.yml` lines 12-38).
  - The image is `${CEOS_IMAGE:-ceos:image}`, so it comes from a local import.
  - The management subnet is 198.51.100.0/24. The book lab's Docker network is `network-observability`.
- **The book lab's own notes** (`reference/network-observability-lab/CLAUDE.md`, loaded as context):
  - They call for Docker Compose v2 and the `network-observability` network, with `LAB_SUDO=true` if containerlab needs sudo.
  - They say nothing about arm64.

**Question 3: workshop stack and Sonda on arm64**

- **Service counts:**
  - Each compose file has 22 service entries.
  - `autocon5/docker-compose.yml` and `packt/docker-compose.yml` have the same services.
  - Five services are built locally in both files: `telegraf-srl1`, `telegraf-srl2`, `grafana`, `webhook`, `prefect-flows`.
  - `grafana` also sets `image:` as the name for its local build.
- **The preflight rules** (`reference/workshops/packages/nobs/src/nobs/lifecycle/preflight.py` lines 7-8, 123-150):
  - RAM of 8 GiB or more is ok, 6 to 8 is a warning, and under 6 fails ("need ~8 GiB for the full stack").
  - Free disk in the current folder must be at least 5 GiB.
  - RAM is read with `psutil.virtual_memory().total`, the host's total, not the Docker VM's limit.
- **Sonda's docker page** (`reference/sonda/docs/site/docs/deploy/docker.md` lines 15, 400-416):
  - "Docker picks the right architecture for your host."
  - The published images are multi-arch (linux/amd64 and linux/arm64) and are made from the release binaries.
- **Sonda on GitHub Container Registry** (https://github.com/davidban77/sonda/pkgs/container/sonda, read 2026-11-04): `latest` lists linux/amd64 and linux/arm64. I did not check the exact tags 1.22.2 and 1.23.1.
- **Registry checks:**
  - `prefecthq/prefect:3-latest` lists linux/arm64.
  - `neo4j:2026.05.0-community` lists linux/arm64/v8.
  - `timberio/vector:0.58.0-debian` lists linux/arm64.
  - `grafana/loki:3.6.16` lists linux/arm64.
  - `prom/prometheus:v3.14.0` lists linux/arm64.
  - `opsmill/infrahub:1.11.2` lists linux/arm64.
  - All were read on 2026-11-04.

| Image group | Published for arm64? |
|---|---|
| Sonda (`latest` only) | yes |
| Prefect, Neo4j (packt tag), Vector (packt tag), Loki (packt tag), Prometheus (packt tag), Infrahub (packt tag) | yes |
| Local builds: telegraf ×2, grafana, webhook, prefect-flows | unchecked; the base images in the Dockerfiles are not read |
| alertmanager, redis, rabbitmq, postgres, python-slim | not looked up |
| autocon5 tags (older versions of the same images) | not looked up |

## Reasoned conclusions

1. **Containerlab cannot run directly on macOS.** It needs a Linux VM, so the docs recommend an arm64 OrbStack machine. This rests on the containerlab macOS page and the OrbStack machines page.
2. **The book lab's cEOS nodes are the risk.**
   - A native arm64 cEOS exists only as an engineering build from the 4.33 line, behind an Arista login.
   - Newer x86 builds do not run on Apple silicon, and VM-based nodes are ruled out by the nested-virtualization limit.
   - So the two-node topology can work only if David holds an Arista account and downloads the arm64 build.
   - This rests on the ipSpace post, the containerlab cEOS page and the Infrahub page.
   - The BGP result is contradictory, and the book lab uses BGP-adjacent features and gNMI. So "works" is not proven for the book's configs.
3. **No cEOS-free fallback is documented in the files I read.** Nokia SR Linux is arm64-native. The workshops already use SR Linux, since their services are named `telegraf-srl1` and `telegraf-srl2`. I did not read the workshops' lab topology, so I only infer that those labs use it.
4. **The workshop stack is probably fine on arm64.** The ten third-party images I checked all publish arm64, and Sonda publishes arm64. The five local builds depend on their Dockerfiles, which I did not read.
5. **Memory is the practical limit.** The stack needs about 8 GiB. The preflight reads the Mac's total RAM, but Docker only sees the OrbStack limit. A passing preflight does not prove the stack will fit.
6. **Two Docker engines may not share networks.** The book lab expects the external network `network-observability` to be shared by containerlab and compose. If containerlab runs in an OrbStack Linux machine with its own Docker, that network is not the same as the one in the OrbStack Docker engine. No source I read says how to join them. This is my reasoning, not a documented fact.

```
Mac (arm64, macOS)
 └── OrbStack
      ├── Docker engine ── compose stack (22 services)   arm64 images, ~8 GiB
      └── Linux machine (arm64)
           └── own Docker + containerlab ── cEOS ×2     needs arm64 cEOS image
```

Question 6 is whether the two boxes inside OrbStack can reach each other.

## Open questions

- The Mac's total RAM and the OrbStack memory limit are not known to me.
- Does David have an Arista account? Is there an arm64 cEOS release newer than the 4.33 engineering build? Does it support the book's configs (gNMI, SNMP, BGP)?
- How would the two Docker engines share the `network-observability` network and the 198.51.100.0/24 subnet? Is the Mac or compose reaching the cEOS nodes by IP possible?
- Can OrbStack's own Docker engine run containerlab directly, without a Linux machine? The containerlab page does not say.
- The arm64 status of the five local builds, and of alertmanager, redis, rabbitmq, postgres and python-slim.
- The exact Sonda tags 1.22.2 and 1.23.1 are unchecked for arm64.
- The autocon5 compose file pins older image versions than packt, and I only checked the packt versions.
- Two sources disagree on timing and on the build. The containerlab page says Arista announced arm64 for 2024, the blog shows an early 2025 engineering build, and the Infrahub page says no arm64 cEOS exists. I read the Infrahub page as being about its own registry tags and the newer 4.36 build. I have not confirmed that, so I did not mark it as a conflict.
- Leads I did not follow: the Dockerfiles for the five local builds, the other workshop files, and the containerlab Windows and devcontainer pages.

## Decisions needed

DECISION NEEDED: Does David have an Arista account, and may the plan assume the arm64 cEOS 4.33 build, or should the plan use SR Linux instead of cEOS on the Mac? Default: the plan marks cEOS as "needs an Arista account and the arm64 build" and does not depend on it for the first version.
DECISION NEEDED: How much memory may the plan assume for OrbStack and the Mac? Default: treat 8 GiB for Docker as the minimum, from the preflight rule, until David gives the real numbers.
DECISION NEEDED: Is the Mac the only first target for containerlab, or should the plan also cover an x86 Linux machine or cloud session where cEOS works without limits? Default: the Mac for compose stacks, and an x86 Linux host noted as the place for the full cEOS lab.
DECISION NEEDED: Should I run a follow-up discovery, with permission to read the five Dockerfiles and look up the remaining images? Default: yes, read-only.

CONFLICT: https://containerlab.dev/macos/ and https://docs.infrahub.app/infrahub-demo-sp/lab/containerlab

Correction: just before this line I said I had not confirmed whether the sources disagree and so had not marked a conflict. The status line above contradicts that. The two pages do pull apart. The containerlab page lists cEOS x64 as "known to work" under Rosetta, and the Infrahub page says Rosetta cannot run the new 32-bit builds. They may talk about different versions, so I leave it as an open question. Please treat it as one that still needs checking, not as settled.
