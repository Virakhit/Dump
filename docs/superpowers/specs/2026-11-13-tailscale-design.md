# Dump + Tailscale: แบบระบบและแผนพัฒนา

วันที่: 2026-11-13 (Asia/Bangkok)
สถานะ: ผู้ใช้อนุมัติพัฒนาเมื่อ 2026-11-13; มี implementation และผลทดสอบอัตโนมัติใน branch `codex/tailscale` แล้ว ตรวจสถานะล่าสุดใน [execution ledger](../plans/2026-11-13-tailscale.md) และ [VERIFICATION](../../VERIFICATION.md) การรับส่งระหว่างเครื่องจริงและข้าม tailnet ยังต้องทดสอบแยก

## เป้าหมายและข้อสรุปจากผู้ใช้

ให้ Dump ส่งไฟล์และแชตผ่าน Tailscale โดยต่อยอด Tauri/React/Rust/libp2p เดิม รองรับทั้งอุปกรณ์ใน tailnet เดียวกันและเครื่องที่แชร์ข้าม tailnet ผู้ใช้เลือกติดตั้งและ login Tailscale แยกจาก Dump แล้วให้ Dump ตรวจสถานะและเชื่อมต่ออัตโนมัติ

Tailscale รับผิดชอบเครือข่ายเสมือนและเส้นทางภายในเครือข่ายนั้น ส่วน Dump ยังคงรับผิดชอบ Device Peer ID, workspace approval, signed membership/manifest, SHA-256, streaming และการจบไฟล์แบบ no-overwrite ผู้ใช้ Tailscale ที่ยังไม่ผ่านการอนุมัติใน Dump ต้องอ่าน catalog, รับไฟล์ หรือแชตไม่ได้

## สิ่งที่พบในโค้ดปัจจุบัน

- `src-tauri/src/network.rs`: มี authenticated QUIC และ TCP/Noise/Yamux แล้ว แต่ listener ปัจจุบันใช้ IPv4 และพอร์ตสุ่ม; discovery ผ่าน mDNS; `preferred_routes` จัดลำดับ direct ก่อน Dump relay
- `src-tauri/src/connectivity.rs`: `direct_address` ยอมรับ literal IP แต่ไม่รับ DNS; `public_address` ตั้งใจตัด CGNAT `100.64.0.0/10` และ IPv6 ULA ออก
- `src-tauri/src/contact.rs`: signed Contact v1 รับเฉพาะ public routes จึงใช้ IP Tailscale ใน contact ปัจจุบันไม่ได้
- `src-tauri/src/network.rs`: ขั้นตอนสร้าง own contact คัดเฉพาะ public addresses; ไม่ได้เผยแพร่เส้นทาง overlay
- `src-tauri/src/streams.rs`: short LAN invitation มีขอบเขต private-address QUIC ของตัวเอง ไม่ควรขยายให้หมายถึง Tailscale
- `src-tauri/src/model.rs`: Invitation ใช้ `deny_unknown_fields` และตรวจ version ร่วมกับโครงสร้างเดิม การเพิ่มฟิลด์หรือเปลี่ยน version กลางโดยตรงเสี่ยงทำให้ invite/state เก่าใช้ไม่ได้
- มีเอกสารแผน Render relay ที่ยังไม่ถูก track อยู่แล้ว งาน Tailscale นี้ไม่ถือว่าแผนนั้นถูกยกเลิกหรืออนุมัติให้พัฒนาต่อ

## แนวทางที่พิจารณา

1. **ใช้ Tailscale ที่ติดตั้งแยก — ผู้ใช้เลือกแนวทางนี้:** เรียก CLI แบบจำกัดคำสั่งเพื่ออ่านสถานะ แล้วส่งข้อมูลผ่าน libp2p เดิม เพิ่มภาระการติดตั้งหนึ่งแอป แต่คงขอบเขต Dump และใช้เครือข่ายที่ผู้ใช้จัดการอยู่แล้ว
2. **ฝัง Tailscale ใน Dump:** ควบคุม onboarding ได้มากกว่า แต่เพิ่มภาระ runtime, login, การอัปเดต และงาน integration กับ Rust/Windows ไม่อยู่ในรุ่นแรกนี้

## ขอบเขตการรองรับเครือข่าย

### Tailnet เดียวกัน

ใช้เส้นทางของอุปกรณ์ที่ local Tailscale มองเห็นและนโยบายเครือข่ายอนุญาต ไม่ถือว่ารายชื่อใน Tailscale คือสมาชิก workspace และไม่เปิดเชื่อมต่อไปทุกเครื่องเพื่อค้นหา Dump

### แชร์เครื่องข้าม tailnet

ใช้ node sharing ของ Tailscale ที่ผู้ใช้ตั้งค่าและยอมรับแล้ว Sharing หนึ่งเครื่องไม่ได้สร้าง full mesh ให้ทุกคนใน workspace และ shared machine เริ่ม connection ย้อนกลับไม่ได้โดยปริยาย

- ฝั่งที่ได้รับ share เป็นฝ่ายเปิด connection ไปยัง shared machine จากนั้น Dump ใช้ authenticated connection เดียวกันเปิด streams รับส่งไฟล์/แชตได้ทั้งสองทิศทาง ต้องพิสูจน์พฤติกรรมนี้กับ libp2p จริงในระยะแรก
- หลัง connection หลุด ให้ฝั่งที่มีสิทธิ์เริ่ม connection เป็นผู้เชื่อมใหม่ โดย retry แบบมีขอบเขต ฝั่งที่เริ่มไม่ได้รอ inbound และแสดงสถานะตามจริง
- สมาชิก B กับ C ซึ่งเห็นเฉพาะเครื่อง A จะไม่เชื่อมถึงกันเอง การรับส่งระหว่าง B/C ต้องมี sharing/policy ที่อนุญาตเส้นทางระหว่างคู่นั้น Dump อธิบายข้อจำกัดและแนะนำ mutual sharing เมื่อจำเป็น; รุ่นนี้ไม่เพิ่มตัวกลางส่งไฟล์ผ่าน A
- IP ของ shared machine อาจถูก remap เพราะชนกับ IP ใน tailnet ผู้รับ ต้องใช้ข้อมูลเส้นทางในเครื่องผู้รับ ไม่ยึด IP ที่ผู้ส่งใส่ใน invitation

## โครงสร้างที่เสนอ

### 1. Tailscale adapter

เพิ่ม `src-tauri/src/tailscale.rs` และแยกส่วนแปลงสถานะ/เส้นทางไว้ใน `src-tauri/src/overlay.rs` หากขนาดโค้ดเหมาะสม

- ตรวจหา executable จากตำแหน่งติดตั้งที่เชื่อถือได้ เรียกด้วย process API และ argument คงที่ ไม่เปิด shell หรือให้ frontend ส่งคำสั่ง arbitrary
- อ่าน `tailscale status --json` แบบ async พร้อม timeout 3 วินาที, output limit 2 MiB, ไม่เปิดหน้าต่าง console และไม่ให้ process ค้างหลังปิดแอป
- refresh เมื่อเปิดแอป ผู้ใช้กดตรวจใหม่ และทุก 15 วินาทีเฉพาะเมื่อเปิดโหมดนี้; จำกัด process พร้อมกันหนึ่งตัวและ backoff เมื่อผิดพลาด
- แยกสถานะ ไม่ติดตั้ง / service ไม่พร้อม / ต้อง login / พร้อมใช้ / ข้อมูลอ่านไม่ได้ ไม่อ้างว่าการอ่าน status สำเร็จหมายถึง Dump ปลายทางเข้าถึงได้
- parser ยอมรับฟิลด์ใหม่จาก CLI แต่ validate ชนิด/ขนาดฟิลด์ที่ใช้; ทดสอบกับ client versions ที่จะประกาศรองรับ เพราะ JSON schema เปลี่ยนได้
- ไม่บันทึก full status, อีเมล หรือรายชื่อทั้ง tailnet ลง log; เก็บ binding เฉพาะอุปกรณ์ Dump ที่เกี่ยวข้องใน DPAPI state เดิม

### 2. Listener และ route policy

ข้อเสนอทางเทคนิคของผู้วางแผน: ใช้ UDP และ TCP **42043** สำหรับ overlay แยกจาก built-in Dump relay 42042 และจาก listener LAN เดิม

- bind เฉพาะ Tailscale IP ของเครื่องที่ local client รายงาน รองรับ IPv4 ก่อน และเพิ่ม listener IPv6 ที่ `fd7a:115c:a1e0::/48` ภายใน milestone เดียวกัน
- ถ้าพอร์ตถูกใช้ แสดงเหตุผลและหยุดเฉพาะ overlay; ไม่สุ่มเปลี่ยนพอร์ตโดยที่คู่เชื่อมต่อไม่ทราบ
- ใช้ QUIC ก่อน และ authenticated TCP fallback; ทั้งคู่ตรวจ expected Device Peer ID เสมอ
- ยืนยันเส้นทางจาก local Tailscale peer map; ช่วง `100.64.0.0/10` อย่างเดียวไม่ใช่หลักฐานว่าเป็น Tailscale เพราะใช้กับ CGNAT อื่นได้
- ไม่เปลี่ยน `public_address` ให้ยอมรับ overlay และไม่ส่ง overlay เข้า public Identify/AutoNAT/DCUtR
- ใช้เส้นทาง Tailscale ที่เลือกไว้เท่านั้นสำหรับ invite ชนิดนี้ ไม่ fallback ไป public IP หรือ Dump relay โดยเงียบ ๆ; ฟังก์ชัน LAN/Internet เดิมยังใช้เส้นทางของตัวเอง
- เก็บนโยบายเฉพาะเครื่องต่อ workspace เป็น `WorkspaceNetworkPolicy::{Legacy,TailscaleOnly}` แยก map ใน persisted state โดย workspace เก่า default เป็น `Legacy`; สวิตช์ระดับแอปเปิด adapter เท่านั้น ส่วนหน้าจอ workspace เลือกนโยบายใช้งาน การเปิดลิงก์ชนิด Tailscale แสดงว่าจะใช้ `TailscaleOnly` และตั้ง pending-join policy ก่อน dial แล้วจึงบันทึกนโยบายเมื่อเข้าร่วมสำเร็จ
- `TailscaleOnly` มีผลกับ join, control, contact refresh, catalog, file และ chat ตลอดอายุการใช้งาน workspace ไม่ใช่แค่ dial แรก; connection/stream selection ต้องแยก overlay จาก direct LAN/public และ relay ตาม Connection ID/endpoint ที่ยืนยันกับ adapter ห้าม reuse connection ของ Peer ID เดียวกันที่อยู่นอก policy ต้องเพิ่ม overlay stream control และตรวจ policy ฝั่ง inbound ด้วย; การเปลี่ยนเส้นทาง tunnel ภายใน Tailscale ไป DERP ยังเป็น overlay ที่อนุญาต
- การเปลี่ยนนโยบาย workspace หรือปิด adapter ต้องรอ transfer ที่เกี่ยวข้องจบก่อนเป็นค่าเริ่มต้น หรือให้ผู้ใช้เลือกยืนยันยกเลิก transfer ตามรูปแบบ UI เดิม แล้วจึงปิด established overlay connections/listeners และล้าง route; ไม่ย้าย transfer ที่กำลังทำไปเส้นทางอื่นโดยเงียบ ๆ
- ติดตาม local IP/peer-map เปลี่ยน, logout และ node หาย; ลบ route เก่าและปิด listener ของ overlay ที่เลิกใช้โดยไม่ reset identity/workspace
- ข้อปรับจากการทดสอบ implementation: ขณะ adapter ยังเปิดแต่ status ใช้ไม่ได้ชั่วคราว ให้ปิด connection/ยกเลิก transfer และปฏิเสธทุก operation โดยคง socket และการจำแนกเส้นทางที่เคยตรวจแล้วไว้จน status พร้อมอีกครั้ง ไม่ประกาศ listener ว่าพร้อมและไม่ dial จากข้อมูลเก่า ปิด socket เมื่อผู้ใช้ปิด adapter หรือเมื่อ status ที่ตรวจใหม่รายงาน IP เปลี่ยน เพื่อหลีกเลี่ยง QUIC drain ถือพอร์ตเดิมระหว่าง logout/relogin
- Windows Firewall และ Tailscale grants ต้องอนุญาต TCP/UDP 42043 บนอุปกรณ์ที่เกี่ยวข้อง มีคู่มือตั้งค่าโดยผู้ดูแล; Dump ไม่เปลี่ยน policy/firewall อัตโนมัติ

### 3. Invitation และ contact ของ overlay

เพิ่มชนิด `OverlayContact` และ invite **`dump://tailscale/<base64url(JSON)>`** แยกจาก `dump://lan/` และ `dump://join/` เพื่อไม่เปลี่ยน validation ของ Contact v1

- envelope version 1 มี tagged `kind` เป็น `invite` หรือ `connect`; แบบ `invite` บรรจุ Invitation เดิมที่ตรวจสอบแล้วและ owner OverlayContact; จำกัดลิงก์รวม 8192 bytes เช่นเดียวกับ full invite เดิม
- OverlayContact มี workspace ID, Device Peer ID/public key, Tailscale node identifier, FQDN, UDP/TCP port, sequence, issued/expiry และลายเซ็นของอุปกรณ์ Dump; domain ของลายเซ็นเป็น `dump/overlay-contact/1`
- อายุ contact ไม่เกิน 24 ชั่วโมง, clock allowance 300 วินาที, route ไม่เกิน 8, cache รวมไม่เกิน 256, payload/frame ไม่เกิน 8192 bytes; ปฏิเสธ field เกินขอบเขต signature ผิด/หมดอายุ/sequence ย้อนหลัง และ contact ข้าม workspace
- FQDN เป็น routing hint ไม่ใช่ identity; map node identifier ผ่าน local peer map เพื่อหา IP ที่ผู้รับใช้จริง หากใช้ DNS fallback ต้องเป็น FQDN ของ peer เดียวกันและผล resolve อยู่ใน local peer map ของ node นั้น
- หาก matching ไม่ชัดเจน ให้รายงานว่าต้องตรวจ sharing หรือสร้าง invitation ใหม่ ห้ามเดาจากชื่อเครื่องหรือเลือก IP ที่บังเอิญตรงกัน
- หลังเชื่อมแล้วตรวจ Device Peer ID อีกชั้น และยังใช้ fingerprint comparison/owner approval เดิม
- แลก OverlayContact เฉพาะสมาชิก workspace ที่ผ่าน authorization ผ่าน protocol `/dump/overlay-contacts/1`; ไม่ broadcast metadata ของ workspace ให้ทั้ง tailnet
- สมาชิก workspace เดิมได้ contact ครั้งแรกผ่าน connection เดิมที่ authorized แล้วและนโยบายขณะนั้นอนุญาต ก่อนเปลี่ยนเป็น TailscaleOnly; หากไม่มี connection เดิม ให้สมาชิกที่ผ่านอนุมัติแล้วสร้างลิงก์ `kind: connect` พร้อม OverlayContact ของตนส่งผ่านช่องทางเดิม ฝั่งเปิดต้องมี workspace อยู่แล้วและตรวจทั้งตนเองกับผู้ลงนามอยู่ใน signed roster ปัจจุบันของเครื่อง ตรวจ public key ตรงสมาชิก และแสดงผลนโยบายก่อนเปิดใช้ ลิงก์นี้ไม่มี invitation token ไม่เพิ่มสมาชิก ไม่ reset roster/identity และไม่ต้องให้ owner ออนไลน์
- cache แยกจาก `Contact` v1 มีค่า default สำหรับ state เดิม ไม่เปลี่ยน global `VERSION` ที่มีผลต่อ identity/roster/state โดยไม่จำเป็น
- build ใหม่ยังอ่าน invite/state เดิมได้; build เก่าไม่รองรับลิงก์ Tailscale จึงต้องระบุว่าอุปกรณ์ทั้งคู่ต้องอัปเดต ไม่อ้าง backward compatibility ของฟีเจอร์ใหม่
- ขั้นพิสูจน์ต้องยืนยันว่า node identifier จาก CLI จับคู่ shared node ข้าม tailnet ได้จริง รวมกรณี remapped IP; หากไม่ได้ ต้องทบทวน locator design ก่อนทำระยะถัดไป

### 4. หน้าจอและการใช้งาน

เพิ่มตัวเลือก **Use installed Tailscale** ใน Network settings พร้อมสถานะและปุ่มตรวจใหม่ ผู้ใช้ login/ยอมรับ machine share ผ่าน Tailscale เอง

ขั้นตอน: เปิด Tailscale ทั้งสองฝั่ง → เปิดโหมดใน Dump → เลือก Invite via Tailscale → ส่งลิงก์ผ่านช่องทางเดิม → อีกฝั่งเปิดลิงก์ → เทียบ fingerprint/อนุมัติ → ส่งไฟล์หรือแชต

แสดง `Via Tailscale` สำหรับเส้นทาง overlay ห้ามใช้ค่า libp2p `Direct` เพื่อกล่าวว่า tunnel เป็น direct เพราะ Tailscale อาจวิ่งผ่าน DERP/peer relay; แสดงรายละเอียด direct/DERP เฉพาะเมื่อมีข้อมูล runtime ยืนยัน มิฉะนั้นให้เป็น Unknown

ข้อความผิดพลาดแยก Tailscale ไม่พร้อม, ไม่พบ shared peer, พอร์ตเข้าไม่ถึง, Peer ID ผิด และ invite หมดอายุ การ timeout ไม่พิสูจน์ว่า ACL บล็อก จึงแนะนำตรวจ permission/firewall โดยไม่ฟันธงสาเหตุ

## แผนแบ่งระยะและเกณฑ์ผ่าน

| ระยะ | งานและจุดแตะโค้ด | เกณฑ์ผ่าน |
|---|---|---|
| 0 — พิสูจน์เครือข่าย | เครื่อง Windows จริง 2 เครื่อง, tailnet เดียวกันและ node sharing ข้าม tailnet; ตรวจ CLI schema/locator, asymmetric initiation, TCP/QUIC, port และ address remapping | เชื่อมโดยไม่ใช้ mDNS, ยืนยัน Peer ID, รับส่งสองทิศทางบน connection ที่ recipient เปิด, reconnect ได้; ถ้า locator ไม่พิสูจน์ต้องแก้แบบก่อนเดินต่อ |
| 1 — อ่านสถานะและเส้นทาง | เพิ่ม `tailscale.rs`, `overlay.rs`, ลงทะเบียน `lib.rs`; settings ใน `relay_host.rs`/`model.rs`, bindings ใน `engine.rs` | fixtures สำหรับไม่ติดตั้ง/logout/timeout/schema ผิด/peer remap/ชื่อซ้ำ/IP CGNAT ที่ไม่ใช่ peer; ไม่เชื่อมเครื่องนอกเป้าหมายและ state เก่าเปิดได้ |
| 2 — เชื่อมต่อและเชิญ | `network.rs`, `streams.rs`, `connectivity.rs`, `model.rs`, `engine.rs`; parser, signed overlay contact, persisted workspace policy และ member-only exchange | invite ใหม่ join/approve ได้ทั้งสอง topology; สมาชิกเดิมเชื่อมผ่าน contact exchange/connect link โดยไม่ join ใหม่; invite เก่าไม่เสีย; wrong Peer ID/contact tamper/replay/outsider ถูกปฏิเสธ; ไม่รั่ว overlay routes ไป public discovery และไม่ reuse connection นอก policy |
| 3 — UI และข้อมูลวินิจฉัย | `desktop.rs`, `engine.rs`, `src/main.tsx`, `scripts/invite-ui.test.cjs`; เพิ่ม integration tests ใน `src-tauri/tests/tailscale.rs` | ผู้ใช้เปิดโหมด/เชิญ/รับไฟล์/แชตได้และเข้าใจสิ่งที่ต้องตั้งค่าใน Tailscale; สถานะไม่อ้าง direct หรือ policy failure โดยไม่มีหลักฐาน |
| 4 — ทดสอบจริงและเตรียมส่งมอบ | เพิ่ม `docs/TAILSCALE.md`, ขยาย `docs/NETWORK_ACCEPTANCE.md`, บันทึกผลใน `docs/VERIFICATION.md` | ผ่าน matrix ด้านล่างทั้งเครื่องจริงและ automated regression แล้วจึงพิจารณา installer/release แยกจากงานวางแผน |

ประมาณการเบื้องต้นสำหรับผู้พัฒนา 1 คน: **8–12 วันทำงาน** เมื่อมีอุปกรณ์/บัญชี/เครือข่ายทดสอบพร้อม รวม buffer สำหรับข้าม tailnet; ประเมินใหม่หลังระยะ 0 หาก remapping หรือ reconnect มีข้อจำกัด ไม่ใช่คำมั่นวันส่งมอบ

## Matrix การตรวจรับ

- สองเครื่อง Windows คนละเครือข่าย แต่ tailnet เดียวกัน: join/approve/file/chat สองทิศทาง; owner ปิดหลังอนุมัติแล้วสมาชิกอื่นที่มีเส้นทางยังทำงานได้
- สอง tailnet แชร์เครื่องทางเดียว: recipient เปิด connection, shared host ส่งไฟล์/แชตกลับบน connection นั้นได้, restart/disconnect แล้ว recipient เชื่อมใหม่; ทดสอบ mutual share แยก
- สามเครื่อง A/B/C: A แชร์ให้ B/C ไม่ถือว่า B/C เชื่อมกันได้; แสดงสถานะตรงจริง แล้วเพิ่ม pairwise sharing ตรวจว่าส่งตรงกันได้
- shared-node IP collision/remapping และ FQDN: ใช้ IP จากมุมมองผู้รับและยังจับคู่ Device Peer ID ถูกต้อง หากไม่มี collision setup จริงให้บันทึกว่า automated-only และยังไม่ผ่าน gate นี้
- IPv4/IPv6, MagicDNS ปิด, port occupied, firewall/grants ไม่อนุญาต, service ปิด, logout/relogin, key expiry และสลับ Wi-Fi/hotspot
- Tailscale direct และ DERP ที่สังเกตได้จริง: ตรวจ hash/throughput/memory, ไฟล์ 0 bytes, ไฟล์เกิน 4 GiB และ cancellation; ไม่เอาเพดาน 256 MiB ของ Dump relay มาแสดงกับเส้นทาง Tailscale
- outsider ที่อยู่ใน tailnet เดียวกันเข้า catalog/download/chat ไม่ได้; wrong Peer ID, signed contact ผิด/หมดอายุ/replay, roster revocation และ stale contact ถูกจัดการตามขอบเขตเดิม
- มี LAN/public/relay connection เปิดอยู่กับ Peer ID เดียวกันแล้วเลือก TailscaleOnly: ทุก operation ต้องใช้ overlay เท่านั้นแม้ overlay หลุด; ตรวจ inbound/control/chat/file ด้วย ไม่ใช่ทดสอบเพียงลำดับ dial
- workspace เดิมเปิดใช้ overlay ผ่าน authorized exchange หรือ `kind: connect` ขณะ owner offline โดย identity/roster/membership ไม่เปลี่ยน; connect link จาก nonmember หรือ workspace อื่นต้องถูกปฏิเสธ
- เปลี่ยน policy/ปิด adapter ขณะ transfer ทำงาน: ค่าเริ่มต้นรอจบ หรือยกเลิกเมื่อผู้ใช้ยืนยัน; ตรวจ established connections ถูกปิดจริง ไม่มี silent route migration
- transfer หลุดไม่ commit ไฟล์ที่ไม่ครบ, ไม่ overwrite, cleanup `.part` เดิม; ไม่อ้าง resume หรือ automatic transfer retry ที่ยังไม่มี
- ถอด/ไม่ติดตั้ง Tailscale แล้ว LAN invite, LAN transfer, chat และ Internet routes เดิมยังทำงานตามเงื่อนไขเดิม

Automated checks ที่ต้องรันเมื่อพัฒนา: `npm run build`, `npm run test:ui`, `npm run test:core`, `cargo check --manifest-path src-tauri/Cargo.toml`, `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`, `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` และ desktop/integration checks ตามชุด CI ปัจจุบัน ผล loopback/fixtures ไม่แทนผลเครื่องจริง

## นอกขอบเขตและการตัดสินใจที่ยังเป็นข้อเสนอ

- ไม่ฝัง VPN, ไม่สร้าง/login บัญชีให้ผู้ใช้, ไม่เปลี่ยน tailnet grants หรือแชร์เครื่องแทนผู้ใช้, ไม่เพิ่ม mandatory Dump backend, exit node, subnet router, Funnel หรือ hosted relay
- ไม่เพิ่ม offline chat sync, resume, automatic retry ของไฟล์ หรือ full-mesh forwarding ผ่านสมาชิกอื่น
- พอร์ต 42043, refresh/timeout limits, รูปแบบ OverlayContact/ลิงก์ และลำดับระยะเป็นข้อเสนอของผู้วางแผน ผู้ใช้ยืนยันแล้วเฉพาะรองรับทั้งสอง topology และติดตั้ง Tailscale แยก
- ก่อนเริ่ม product code ให้ตรวจทานเอกสารนี้; การทดสอบจริงต้องมีเครื่อง/บัญชี/เครือข่ายที่ผู้ใช้อนุญาตและตัวอย่าง sharing/policy ที่ใช้จริง

## แหล่งอ้างอิงทางเทคนิค

- [Tailscale CLI — JSON format อาจเปลี่ยน](https://tailscale.com/docs/reference/tailscale-cli)
- [Sharing — quarantine, visibility, remapped IP, mutual sharing และ FQDN](https://tailscale.com/docs/features/sharing)
- [Reserved addresses — IPv4 และ IPv6 ของ Tailscale](https://tailscale.com/docs/reference/reserved-ip-addresses)
- [Connection types — direct, DERP และ peer relay](https://tailscale.com/docs/reference/connection-types)
- [Grants syntax — TCP/UDP และ port restrictions](https://tailscale.com/docs/reference/syntax/grants)
- [Client metrics — multicast packets ถูก drop](https://tailscale.com/docs/reference/tailscale-client-metrics)

การตรวจในงานวางแผนนี้: อ่าน source และเอกสารข้างต้นเท่านั้น ยังไม่มีการติดตั้ง Tailscale, เปลี่ยนเครือข่าย, แก้ product code, รัน regression หรือทดสอบ multi-device
