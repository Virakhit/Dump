# Dump 1.0 Render Relay Implementation Plan

> **For agentic workers** — Use `superpowers:executing-plans` or `superpowers:subagent-driven-development` when implementing. Record the build, test environment and gate result at the end of each phase.

**Goal** — ให้ผู้ใช้ Dump บน Windows สองเครื่องที่อยู่คนละเครือข่ายใช้ flow เดิมเพื่อเชิญ อนุมัติ แชร์ และรับไฟล์ โดย C# บน Render ทำ Circuit Relay v2 เมื่อเส้นทางตรงใช้ไม่ได้

**Architecture** — Dump desktop ใช้ Rust/libp2p ตามเดิมและเพิ่ม WSS สำหรับ relay hop ส่วน ASP.NET Core ให้บริการ libp2p Circuit Relay v2 และส่งต่อ stream ระหว่าง peer ปลายทาง ตัว desktop ทั้งสองยังเจรจา Noise และ Yamux กันเองภายใน circuit

**Tech stack** — .NET 10 LTS, ASP.NET Core, Docker Linux, Render Web Service, Tauri 2, React, Rust/libp2p 0.57.0 ตาม lockfile เดิมเป็นจุดเริ่มต้น

**Spec** — ข้อกำหนดด้านล่างต่อยอดจาก [แผนเดิม](../../PLAN.md), [โปรโตคอล](../../PROTOCOL.md) และ [security model](../../../SECURITY.md) ณ checkout `05e1569` เอกสารนี้เป็นแผนงาน ทุก gate ด้านล่างยังรอผลทดสอบ

## ขอบเขตและการตัดสินใจ

- C# รับผิดชอบการจอง relay, เปิด circuit และส่งต่อข้อมูลเอง ใช้ libp2p Circuit Relay v2 มาตรฐานเพื่อคงเส้นทาง `Noise + Yamux` และโปรโตคอลของ Dump ปัจจุบัน Roadmap ของ Nethermind ระบุ Circuit Relay v2 ว่าอยู่ระหว่างพัฒนา จึงต้องพิสูจน์กับ Rust จริงก่อนเลือกว่าจะใช้แพ็กเกจที่ pin ไว้หรือเติมส่วน HOP/STOP ที่ขาดใน C# หากงานลามไปถึงการสร้าง Noise/Yamux ใหม่ ให้หยุดที่ผล spike และประเมิน scope ใหม่ [Nethermind roadmap](https://github.com/NethermindEth/dotnet-libp2p#roadmap) · [Circuit Relay v2 spec](https://github.com/libp2p/specs/blob/master/relay/circuit-v2.md)
- เรียงเส้นทาง LAN → Internet direct → relay เมื่อพร้อม ถ้า DCUtR อัปเกรดเป็น direct ได้ ให้ stream ใหม่ใช้ direct และปล่อย stream เดิมให้จบ ถ้าอัปเกรดไม่ได้ ให้ใช้ circuit ต่อในขอบเขตของมัน การทำงานของ DCUtR ผ่าน relay นี้เป็นสมมติฐานที่ต้องทดสอบ
- ทั้งสองเครื่องเปิด WSS ออกไปที่ Render ผ่านพอร์ต 443 ใช้ TLS ปกติและ pin Relay Peer ID ในการเจรจา libp2p ส่วน Noise ชั้นในตรวจ Device Peer ID ของคู่สนทนา Render ยุติ TLS ที่ edge ได้ แต่จะเห็นเพียง ciphertext ของข้อมูลใน circuit รวมถึง metadata สำหรับ routing เช่น IP, Peer ID, เวลา และปริมาณข้อมูล [Render Web Services](https://render.com/docs/web-services) · [Render WebSockets](https://render.com/docs/websocket)
- ใช้ full invite และ signed contact รูปแบบเดิม เก็บคู่ `hostname + Relay Peer ID` ใน config ที่มากับ build หรือกำหนดในเครื่องผ่าน Advanced settings ผู้ใช้เปิด Internet relay แบบ opt-in เมื่อใช้ full invite ให้ derive circuit จาก owner Peer ID ใน invite และ config ในเครื่อง หลังอนุมัติให้ derive เฉพาะสมาชิกใน roster ของ workspace ที่ใช้งานอยู่ ใส่ WSS route ในรายการ dial ภายในเครื่องเท่านั้น การแชร์ข้ามเน็ตผ่าน hosted relay ต้องใช้ build ที่รองรับทั้งสองฝั่ง
- เก็บ DPAPI identity, workspace, signed roster, การอนุมัติด้วย fingerprint, signed manifest, SHA-256 และ `.part` / no-overwrite เดิม ใช้ short LAN invite และ offline LAN ได้ตามเดิม ลิงก์ LAN จะไม่ trigger การ dial ผ่าน hosted relay
- เริ่มด้วย relay instance เดียวและกลุ่มทดสอบแบบ allowlist ของ Device Peer ID ซึ่งตั้งค่าโดยผู้ดูแล Render ทั้งผู้ส่งและผู้รับต้องอยู่ใน allowlist เพื่อใช้ circuit ใน pilot ทั้งคู่ยังต้องผ่าน workspace approval ตามเดิม การเปิด hosted relay ให้ผู้ใช้ทั่วไปเป็นการตัดสินใจแยกก่อนประกาศขอบเขต 1.0
- ไฟล์อยู่บนเครื่องผู้ส่งและผู้รับ Relay เก็บเฉพาะ reservation, circuit และตัวนับที่มีขอบเขตใน RAM ส่ง ciphertext แบบ streaming พร้อม backpressure งานนี้ครอบคลุมการส่งไฟล์แบบออนไลน์ โดยใช้ manual retry ของ Dump เมื่อ transfer ขาด

## Roadmap

| Phase | งานหลัก | สิ่งส่งมอบ | ต้องผ่านก่อนเริ่ม |
| --- | --- | --- | --- |
| 0 | พิสูจน์ C# ↔ Rust ผ่าน WSS | Docker spike และผล interop บนเครื่องกับ Render | มี test clients และ Render staging สำหรับ smoke test |
| 1 | พัฒนา C# relay สำหรับกลุ่มทดสอบ | Relay ที่จำกัดสิทธิ์และทรัพยากร พร้อม automated tests | Phase 0 |
| 2 | ผูก relay เข้ากับ Dump | Build ที่เชิญ อนุมัติ แชร์ และรับผ่าน hosted relay ได้ | Phase 1 |
| 3 | ทดสอบบน Render และ Windows จริง | ผลทดสอบข้ามเครือข่าย ความปลอดภัย และการใช้ทรัพยากร | Phase 2 |
| 4 | ทดลองกับผู้ใช้ 5 คน | รายงาน pilot และข้อเสนอขอบเขต hosted relay ของ 1.0 | Phase 3 |
| 5 | เตรียมและตรวจ release | Windows installer ที่ตรวจแล้ว พร้อม signed updater และ release notes | Phase 4 |

ทุก Phase อยู่ในสถานะวางแผน เมื่อปิด Phase ให้บันทึก `PASS`, `FAIL` หรือ `NOT RUN` พร้อมหลักฐานตามเกณฑ์ท้าย Phase ถ้า gate ไม่ผ่าน ให้แก้แล้วทดสอบซ้ำก่อนเริ่มงานที่พึ่ง gate นั้น

## แผนราย Phase

### Phase 0 — พิสูจน์ C# ↔ Rust ผ่าน WSS

**จุดแตะโค้ด** — `relay/Dump.Relay/` สำหรับ C# host, `relay/Dump.Relay/Dockerfile` สำหรับ Linux image, `src-tauri/Cargo.toml` สำหรับ WSS/DNS feature, `src-tauri/src/network.rs` สำหรับประกอบ transport และ `src-tauri/tests/relay_wss.rs` สำหรับ Rust ↔ C# integration

- [ ] ตรวจ release และ source ของไลบรารี .NET ที่จะ pin บันทึกผลรองรับ WebSocket binary stream, multistream-select, Noise, Yamux, Identify, HOP `RESERVE`/`CONNECT` และ STOP สรุปช่องว่าง รวมถึงวิธีต่อ WebSocket จาก Kestrel เข้า libp2p stack
- [ ] ทำ ASP.NET Core host สำหรับ spike มี `GET /healthz` และ WebSocket upgrade ที่ `/` ใช้ `PORT` บน `0.0.0.0` ประกาศ public WSS multiaddr ตามรูปแบบที่ Rust อ่านได้ เช่น `/dns4/<hostname>/tcp/443/wss/p2p/<relay-peer-id>` ยืนยัน hostname, SNI และรูปแบบ multiaddr จริงใน smoke test
- [ ] เขียน `csharp_relay_wss_reserve_connect_and_transfer` ให้ Rust A จองกับ C# และ Rust B เปิด circuit ถึง A บังคับ relay-only ใน test harness ตรวจ renewal, HOP/STOP และปลายทาง Noise Peer ID ส่งไฟล์สังเคราะห์ 32 MiB ผ่าน Dump transfer path แล้วเทียบ SHA-256
- [ ] เขียน `csharp_relay_wss_rejects_wrong_peer` สลับ Relay Peer ID แล้วต้องต่อ relay hop ไม่สำเร็จ จากนั้นใช้ test relay ที่จงใจส่ง circuit ไป peer ที่สามซึ่งถือ key คนละตัว ต้องล้มเหลวที่ Noise ชั้นในก่อนเข้าถึงโปรโตคอล Dump
- [ ] รัน `cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --locked --test relay_wss` กับ C# host ที่ระบุด้วย `DUMP_RELAY_TEST_MULTIADDR` ให้ harness ใช้ identities ทดสอบที่ลง allowlist ไว้ ทำ smoke ซ้ำผ่าน Docker Web Service บน Render staging โดยใช้ TLS certificate จริงและไฟล์ทดสอบเท่านั้น

**ผ่านเมื่อ** — `RESERVE`, renewal, `CONNECT` และ STOP ผ่านจาก Rust จริง ไฟล์ 32 MiB ตรงทั้งขนาดและ SHA-256 ทดสอบ key ผิดแล้วถูกปฏิเสธ บันทึก revision, package version, public route shape และผล local/Render แยกกัน ถ้า spike ติดที่ upstream ให้สรุปส่วน C# ที่ต้องทำเพิ่มและปรับแผนตามผลก่อนเข้า Phase 1

### Phase 1 — พัฒนา C# relay สำหรับ pilot

**จุดแตะโค้ด** — `relay/Dump.Relay/Program.cs`, `RelayOptions.cs`, `CircuitRelayService.cs` และ `relay/Dump.Relay.Tests/` แยก HTTP host, startup config และ reservation/circuit lifecycle ตามหน้าที่

- [ ] ใช้ Relay Ed25519 identity คงที่จาก Render secret และแยกจาก Device identity ถ้า key หรือ public address ตั้งค่าผิดให้ startup fail ก่อน `/healthz` พร้อมใช้งาน ตรวจว่า restart และ rollback ยังได้ Peer ID เดิม
- [ ] รับ RESERVE/CONNECT จาก libp2p Peer ID ที่ผ่าน Noise และอยู่ใน allowlist ใช้เฉพาะ session ที่มี reservation ตรงกับปลายทางและยังไม่หมดอายุ ห้ามให้ผู้ขอส่ง URL หรือ IP เพื่อให้ server เปิด socket ตามคำสั่ง เก็บ voucher และ status ตาม Circuit Relay v2
- [ ] ตั้งเพดานเริ่มต้นที่ 16 reservations, 1 ต่อ peer, อายุ 10 นาที, 8 circuits, สูงสุด 2 ต่อ peer โดยนับทั้งต้นทางและปลายทาง, 10 นาที และ 256 MiB ของข้อมูลรวมสองทิศต่อ circuit ค่าพวกนี้วัดรวม framing และ traffic ใน circuit จึงใช้เป็นเพดานขนาดไฟล์โดยตรงไม่ได้ จำกัด pending/established connections, upgrade rate และเวลา handshake ก่อนจัดสรร stream เพิ่ม [ข้อจำกัดของ host เดิม](../../PROTOCOL.md#opt-in-hosting-stage-5)
- [ ] ใช้ bounded buffer ต่อทิศและเพดาน throughput รวมที่ปรับจาก config ได้ เริ่มวัดที่ 2 MiB/s สำหรับ bytes ที่ส่งต่อรวมทั้งสองทิศ บันทึก counters สำหรับ rejected, active, bytes, quota และ disconnect reason โดยไม่บันทึก payload, invite, workspace, filename, key หรือ Peer ID แบบรายตัว
- [ ] เขียน test สำหรับ slot สุดท้ายถูกแย่งพร้อมกัน, reservation หมดอายุ/renew, stale connection cleanup หลัง peer ต่อใหม่, frame ผิดรูปหรือใหญ่เกิน, slow reader, circuit หมด byte/time, และ disconnect/redeploy กลาง stream ใช้ค่าขอบเขตเล็กใน test เพื่อทดสอบ overflow และ race ได้เร็ว
- [ ] รัน `dotnet test relay/Dump.Relay.Tests/Dump.Relay.Tests.csproj` และ Rust ↔ C# suite ยืนยันว่า slots และ buffers กลับสู่ baseline หลังทดสอบซ้ำ ให้ test harness ตรวจ ciphertext ฝั่ง relay ด้วย synthetic marker โดยไม่เก็บ payload ใน runtime logs

**ผ่านเมื่อ** — ใช้ครบโควตาแล้วปฏิเสธงานใหม่ตามสถานะที่โปรโตคอลรองรับ ทรัพยากรถูกคืนหลังหมดอายุหรือ disconnect และ peer นอก allowlist เปิด reservation หรือ circuit ไม่ได้

### Phase 2 — เชื่อม relay เข้ากับ Dump Desktop

**จุดแตะโค้ด** — `src-tauri/src/connectivity.rs` สำหรับ validate/ประกอบ managed route, `src-tauri/src/network.rs` สำหรับ reserve/retry/route, `src-tauri/src/relay_host.rs` และ `src-tauri/src/engine.rs` สำหรับ config/state, `src-tauri/src/desktop.rs` กับ `src/main.tsx` สำหรับสถานะและ full-invite UX เพิ่ม regression ใน `src-tauri/tests/relay_wss.rs` และ `scripts/invite-ui.test.cjs`

- [ ] แยก WSS validation ออกจาก `direct_address` และ `public_address` ใช้ hostname, พอร์ต 443 และ Relay Peer ID ตรงกับ managed config เท่านั้น เปิด hostname จาก config ที่เชื่อถือได้และใช้ TLS ปกติ ทดสอบว่าการรับ full invite, signed contact และ Identify จาก peer ไม่ขยายสิทธิ์ DNS dial หรือส่ง DNS เข้า AutoNAT/DCUtR candidate set
- [ ] เพิ่ม route เฉพาะจาก config ที่ opt-in และเป้าหมายที่อยู่ใน full-invite join หรือ current signed roster เก็บ route slot สำหรับ managed fallback ภายในเพดานเดิม 8 routes ต่อ peer รอ accepted reservation ก่อนแสดงว่า relay ของเครื่องพร้อมรับ incoming circuit ให้ผู้สร้าง full invite เห็นสถานะนั้นหรือ direct route ที่ใช้ได้ และอธิบายเงื่อนไขกรณีทั้งสองเครื่องใช้ hosted relay
- [ ] ใช้ exponential backoff + jitter เมื่อ relay หลุด คืน reservation และเปิด circuit ใหม่ได้เมื่อ service กลับมา การต่อใหม่ไม่ restart transfer ที่ค้างอัตโนมัติ แสดงสถานะ Direct / Via relay / Reconnecting / Relay unavailable ตามผลจริง และใช้สาเหตุ quota เฉพาะเมื่อยืนยันได้
- [ ] ทดสอบ LAN + short invite เมื่อปิด Internet, full invite ข้ามเครือข่าย, fingerprint approval, catalog และ receive, old profile/old full invite/legacy relay compatibility, outsider ที่ผ่าน relay allowlist แต่ยังไม่เป็นสมาชิก, member revocation และ relay drop กลางไฟล์ ตรวจว่า `.part` ที่ Dump เป็นเจ้าของถูกล้าง ไม่มีไฟล์เสีย commit และผู้รับกดรับใหม่ได้หลัง reconnect
- [ ] รัน Rust core suite และ UI tests ตาม [DEVELOPMENT.md](../../DEVELOPMENT.md) ทดสอบ direct upgrade และการที่ active relay stream ส่งจบได้ ทั้งกรณี upgrade สำเร็จและกรณี UDP ถูกบล็อก

**ผ่านเมื่อ** — ผู้ใช้เข้าผ่าน full invite และแชร์/รับได้ผ่านหน้าจอเดิม การเปิด relay ใช้ได้โดยไม่สร้าง device key หรือ workspace ใหม่ และ LAN ไม่พึ่ง availability ของ Render

### Phase 3 — ทดสอบบน Render และ Windows จริง

**จุดแตะโค้ด** — `render.yaml`, `relay/Dump.Relay/Dockerfile`, `.github/workflows/check.yml` และ `docs/RENDER_RELAY.md` สำหรับขั้นตอน deploy, secrets, rotation, rollback และ recovery

- [ ] ใช้ Docker Web Service, health check `/healthz` และ instance เดียว ทดสอบ `SIGTERM` ให้หยุดรับ circuit ใหม่ ยกเลิกงานที่ยังไม่จบเมื่อถึง shutdown deadline และให้ client reconnect ใช้ paid instance สำหรับ pilot ที่ต้องพร้อมรับคำขอหลัง idle ส่วน Free ใช้ smoke test และบันทึก cold start [Blueprint](https://render.com/docs/blueprint-spec) · [Health checks](https://render.com/docs/health-checks) · [Free tier](https://render.com/docs/free)
- [ ] วัด RSS, active circuits, latency, throughput, reconnect และ egress ด้วยไฟล์สังเคราะห์ 32/128 MiB ที่ 1 circuit และช่วง concurrent ตามเพดาน pilot ใช้ Render bandwidth metrics เทียบกับตัวนับ relay ตั้ง threshold แจ้งเตือนและขั้นตอนปิดรับ circuit ใหม่ ตัวนับใน RAM จะเริ่มใหม่หลัง restart จึงใช้เป็น hard monthly quota ไม่ได้ [Render bandwidth](https://render.com/docs/outbound-bandwidth)
- [ ] รัน [physical-network matrix](../../NETWORK_ACCEPTANCE.md) ด้วย Windows จริงสองเครื่องบน Wi-Fi บ้านและมือถืออีกเครือข่าย เพิ่มกรณี UDP ถูกบล็อก, direct upgrade ที่ยังมีไฟล์ส่งอยู่, restart/redeploy, relay เต็ม, และสามเครื่องสำหรับ revoked-member propagation บันทึก actual route ต่อ transfer, expected Peer ID, ผล hash และผล cleanup แยกจาก CI/loopback
- [ ] ปิด acceptance gates เดิมสำหรับ LAN บนเครื่องจริง, direct transfer ขนาด 8 GiB พร้อมวัดหน่วยความจำ และ privacy/packet capture ใช้ไฟล์สังเคราะห์ แยกหลักฐานเครื่องจริงออกจาก test loopback [แผนและ release gates เดิม](../../PLAN.md#remaining-external-acceptance-gates)

**ผ่านเมื่อ** — เครื่องจริงรับไฟล์ที่ตรวจ SHA-256 ผ่านบน relay-only route, relay outage ทำให้เกิด failure ที่อธิบายได้และ LAN ยังทำงาน, ไม่มี partial commit หรือ memory โตตามขนาดไฟล์ และมีหลักฐาน capacity/cost สำหรับกลุ่ม pilot

### Phase 4 — Pilot กับผู้ใช้จริง

- [ ] เตรียม Windows x64 preview build และคู่มือสั้นสำหรับเชิญ อนุมัติ และรับไฟล์ เพิ่ม Device Peer ID ของผู้เข้าร่วมใน allowlist
- [ ] ให้ผู้ใช้ 5 คนลอง flow ตั้งแต่ติดตั้งจนรับไฟล์ข้ามเครือข่าย บันทึกจำนวนคนที่ทำได้ด้วยตัวเองและกลับมาโอนซ้ำในสองสัปดาห์ พร้อมจำแนกปัญหา install, invite, relay และ transfer [เกณฑ์ pilot เดิม](../../PLAN.md#milestones)
- [ ] เทียบ egress และภาระงานดูแลกับผล Phase 3 แก้ปัญหาที่ทำให้เข้าร่วม แชร์ หรือรับไฟล์ไม่ได้ แล้วทดสอบซ้ำกับ build ที่แก้แล้ว

**ผ่านเมื่อ** — อย่างน้อย 4 จาก 5 คนทำ flow สำเร็จด้วยตัวเอง อย่างน้อย 2 คนกลับมาโอนจริงในสองสัปดาห์ และปัญหาที่ขวาง flow หลักได้รับการแก้และตรวจซ้ำ

### Phase 5 — Release Dump 1.0

- [ ] ตัดสินใจจากหลักฐาน pilot ว่า hosted relay ของ 1.0 จำกัดกลุ่มหรือเปิดทั่วไป ถ้าจะเปิดทั่วไป ให้กำหนด admission, งบ egress และเกณฑ์ abuse/load ก่อนเปิด ส่วนการติดตั้งและใช้ LAN ของ Dump ทดสอบแยกจาก hosted relay
- [ ] สร้าง draft Windows x64 NSIS และ signed updater ตาม [DEVELOPMENT.md](../../DEVELOPMENT.md) ทดสอบ clean install, deep link ตอน app เปิดและปิด และ upgrade จากรุ่นเดิมโดยคง DPAPI identity/workspace
- [ ] ทำ release notes อธิบาย direct/relay, เงื่อนไขให้เครื่องต้นทางเปิดอยู่, ข้อจำกัดวงจร, supported upgrade และวิธีปิด Internet relay
- [ ] ยืนยันผล CI ของ revision ที่ใช้สร้าง installer ตรวจ checksum และ updater signature แล้วตรวจผล gate Phase 3–4 ประกอบการตัดสินใจปล่อย

**ผ่านเมื่อ** — installer และ signed update ทำงานบนเครื่องทดสอบ ข้อมูลเดิมอ่านได้หลังอัปเกรด และระบุชัดว่า hosted relay ของรุ่น 1.0 เปิดให้กลุ่มใด

## ค่าใช้จ่ายและลำดับเวลา

เริ่มทดสอบด้วย Render instance เดียว รุ่น `0.5c-512mb` เป็นตัวเลือกสำหรับ paid pilot หลังวัดโหลด Render คิดค่า compute แยกจาก bandwidth และคิด outbound traffic ที่ส่งจาก relay ไปผู้รับโดยอิงโควตารวมของ workspace หน้าเอกสารระบุ Hobby รวม 5 GB/เดือนและส่วนเกิน public egress $0.15/GB ให้เทียบราคาจาก Dashboard ก่อนเปิด paid pilot [Compute plans](https://render.com/docs/compute-plans) · [Pricing](https://render.com/pricing) · [Outbound bandwidth](https://render.com/docs/outbound-bandwidth)

ตั้งกรอบ Phase 0 ไว้ 3–5 วันทำงานเมื่อมี Render staging กับ test clients พร้อม ประเมินเวลา Phase 1–3 หลังทราบช่องว่าง HOP/STOP และผล interop จริง Phase 4 ใช้เวลาสังเกตผลสองสัปดาห์ วันปล่อย 1.0 ขึ้นกับผลเครื่องจริงและ release gates
