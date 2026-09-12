"""Substrate abstraction for MIOA organisms.

A MIOA organism is not "FBA0 plus organs"; it is

    organism
      |- substrate(s)     inherited biological (or synthetic) tissue
      |- organs           grown artificial components
      |- transducers      environment -> organism signal paths
      |- internal state   homeostatic variables, buffers, debt
      |- effectors        organism -> environment action paths

FBA0 — the FlyWire v783 connectome under the Shiu et al. (2024) LIF
model — is the *ancestral* substrate: the founder condition every
M-series lineage starts from, and one implementation of the substrate
contract. It is not the definition of a MIOA organism.

This package holds the contract (``base``), the generic endpoint
references attachments resolve through (``endpoints``), the registry
development and mutation consult (``registry``) and the FBA0
implementation of the contract (``fba0``). Simulator state stays behind
the FBA backends (``fba/``); a substrate adapter describes the tissue
and mediates lesions, it does not simulate it.
"""
from .base import (LesionSpec, PortSpec, SubstrateProtocol)
from .endpoints import (ENDPOINT_KINDS, EndpointRef, is_substrate_endpoint,
                        parse_endpoint)
from .registry import SubstrateRegistry, adapter_for, default_registry

__all__ = ["ENDPOINT_KINDS", "EndpointRef", "LesionSpec", "PortSpec",
           "SubstrateProtocol", "SubstrateRegistry", "adapter_for",
           "default_registry", "is_substrate_endpoint", "parse_endpoint"]
