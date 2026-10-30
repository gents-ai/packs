## The worked example: radio equipment certification and Flipper Zero

Radio certification is the cleanest demonstration that the paper model fails on exactly the three properties in section 1: the attestation is not bound to the firmware, revocation of a product's compliance has no channel, and nobody can see a device drift from its certified state. The regulators know this; the EU tried for eight years to fix it in law and gave up in January 2026.

**What the Flipper Zero certificate actually says.** FCC ID 2A2V6-FZ, granted 14 Jan 2022 to Flipper Devices Inc. It certifies Bluetooth at 2.4 GHz, a 13.56 MHz NFC device, and sub-GHz transmission under Part 15.231 (periodic remote-control rules) in three windows only: 304.5-321.95, 433.075-434.775 and 915-927.95 MHz. Public exhibits include the manual, photos and test reports; the schematic, block diagram and operational description are under long-term confidentiality. No SDR declaration appears in the exhibit list ([fccid.io mirror of the grant](https://fccid.io/2A2V6-FZ)). A certification grant is issued by an FCC-recognised Telecommunication Certification Body on accredited lab data and posted to the public database ([FCC](https://www.fcc.gov/node/23689)). So the attestation exists, is public, and names the grantee; what it does not name is a firmware hash.

**The rules already require the binding; the mechanism cannot deliver it.**

| Rule | What it requires | Source |
| --- | --- | --- |
| 47 CFR 15.15(b) | A device must be designed so no user-accessible control can take it outside the rules | [LII](https://www.law.cornell.edu/cfr/text/47/15.15) |
| 47 CFR 2.944 | For software-defined radios, only approved software may load, enforced by authenticated download, signatures or hardware codes; if third parties are expected to modify RF-controlling software, the device must be certified as an SDR | [eCFR](https://www.ecfr.gov/current/title-47/chapter-I/subchapter-A/part-2/subpart-J/subject-group-ECFRd5ad3b739dbf27a/section-2.944) |
| 47 CFR 2.1043 | Permissive changes: Class I (no filing), Class II (grantee files test data), Class III (SDR software exceeding approved limits, new test data); software changes not affecting RF may be made by anyone without filing | [LII](https://www.law.cornell.edu/cfr/text/47/2.1043) |
