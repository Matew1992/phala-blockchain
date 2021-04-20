// Copyright (c) The Libra Core Contributors
// SPDX-License-Identifier: Apache-2.0

//! Rust representation of a Move transaction script that can be executed on the Libra blockchain.
//! Libra does not allow arbitrary transaction scripts; only scripts whose hashes are present in
//! the on-chain script allowlist. The genesis allowlist is derived from this file, and the
//! `Stdlib` script enum will be modified to reflect changes in the on-chain allowlist as time goes
//! on.

use anyhow::{anyhow, Error, Result};
use diem_crypto::HashValue;
use diem_types::transaction::{ScriptABI, SCRIPT_HASH_LENGTH};
use std::{convert::TryFrom, fmt};
use std::{string::{String, ToString}, vec::Vec};

const CHILD_ABI: &str = r#"196372656174655f6368696c645f766173705f6163636f756e74b12720232053756d6d6172790a20437265617465732061204368696c642056415350206163636f756e7420776974682069747320706172656e74206265696e67207468652073656e64696e67206163636f756e74206f6620746865207472616e73616374696f6e2e0a205468652073656e646572206f6620746865207472616e73616374696f6e206d757374206265206120506172656e742056415350206163636f756e742e0a0a202320546563686e6963616c204465736372697074696f6e0a2043726561746573206120604368696c645641535060206163636f756e7420666f72207468652073656e6465722060706172656e745f766173706020617420606368696c645f6164647265737360207769746820612062616c616e6365206f660a20606368696c645f696e697469616c5f62616c616e63656020696e2060436f696e547970656020616e6420616e20696e697469616c2061757468656e7469636174696f6e206b6579206f660a2060617574685f6b65795f707265666978207c206368696c645f61646472657373602e0a0a20496620606164645f616c6c5f63757272656e636965736020697320747275652c20746865206368696c6420616464726573732077696c6c20686176652061207a65726f2062616c616e636520696e20616c6c20617661696c61626c650a2063757272656e6369657320696e207468652073797374656d2e0a0a20546865206e6577206163636f756e742077696c6c2062652061206368696c64206163636f756e74206f6620746865207472616e73616374696f6e2073656e6465722c207768696368206d75737420626520610a20506172656e742056415350206163636f756e742e20546865206368696c64206163636f756e742077696c6c206265207265636f7264656420616761696e737420746865206c696d6974206f660a206368696c64206163636f756e7473206f6620746865206372656174696e6720506172656e742056415350206163636f756e742e0a0a202323204576656e74730a205375636365737366756c20657865637574696f6e2077697468206120606368696c645f696e697469616c5f62616c616e6365602067726561746572207468616e207a65726f2077696c6c20656d69743a0a202a204120604469656d4163636f756e743a3a53656e745061796d656e744576656e74602077697468207468652060706179657260206669656c64206265696e672074686520506172656e742056415350277320616464726573732c0a20616e64207061796565206669656c64206265696e6720606368696c645f61646472657373602e205468697320697320656d6974746564206f6e2074686520506172656e74205641535027730a20604469656d4163636f756e743a3a4469656d4163636f756e7460206073656e745f6576656e7473602068616e646c652e0a202a204120604469656d4163636f756e743a3a52656365697665645061796d656e744576656e7460207769746820746865202060706179657260206669656c64206265696e672074686520506172656e742056415350277320616464726573732c0a20616e64207061796565206669656c64206265696e6720606368696c645f61646472657373602e205468697320697320656d6974746564206f6e20746865206e6577204368696c6420564153505327730a20604469656d4163636f756e743a3a4469656d4163636f756e7460206072656365697665645f6576656e7473602068616e646c652e0a0a202320506172616d65746572730a207c204e616d6520202020202020202020202020202020202020207c20547970652020202020202020207c204465736372697074696f6e2020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a207c202d2d2d2d2d2d2020202020202020202020202020202020207c202d2d2d2d2d2d202020202020207c202d2d2d2d2d2d2d2d2d2d2d2d2d202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a207c2060436f696e547970656020202020202020202020202020207c20547970652020202020202020207c20546865204d6f7665207479706520666f72207468652060436f696e5479706560207468617420746865206368696c64206163636f756e742073686f756c64206265206372656174656420776974682e2060436f696e5479706560206d75737420626520616e20616c72656164792d726567697374657265642063757272656e6379206f6e2d636861696e2e207c0a207c2060706172656e745f766173706020202020202020202020207c2060267369676e657260202020207c20546865207369676e6572207265666572656e6365206f66207468652073656e64696e67206163636f756e742e204d757374206265206120506172656e742056415350206163636f756e742e20202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a207c20606368696c645f61646472657373602020202020202020207c20606164647265737360202020207c2041646472657373206f662074686520746f2d62652d63726561746564204368696c642056415350206163636f756e742e20202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a207c2060617574685f6b65795f70726566697860202020202020207c2060766563746f723c75383e60207c205468652061757468656e7469636174696f6e206b65792070726566697820746861742077696c6c206265207573656420696e697469616c6c7920666f7220746865206e65776c792063726561746564206163636f756e742e202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a207c20606164645f616c6c5f63757272656e6369657360202020207c2060626f6f6c60202020202020207c205768657468657220746f207075626c6973682062616c616e6365207265736f757263657320666f7220616c6c206b6e6f776e2063757272656e63696573207768656e20746865206163636f756e7420697320637265617465642e20202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a207c20606368696c645f696e697469616c5f62616c616e636560207c20607536346020202020202020207c2054686520696e697469616c2062616c616e636520696e2060436f696e547970656020746f206769766520746865206368696c64206163636f756e74207768656e206974277320637265617465642e20202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a0a202320436f6d6d6f6e2041626f727420436f6e646974696f6e730a207c204572726f722043617465676f727920202020202020202020202020207c204572726f7220526561736f6e2020202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c204465736372697074696f6e2020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a207c202d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2020202020202020202020207c202d2d2d2d2d2d2d2d2d2d2d2d2d2d202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c202d2d2d2d2d2d2d2d2d2d2d2d2d202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a207c20604572726f72733a3a494e56414c49445f415247554d454e546020207c20604469656d4163636f756e743a3a454d414c464f524d45445f41555448454e5449434154494f4e5f4b4559602020202020202020202020207c205468652060617574685f6b65795f7072656669786020776173206e6f74206f66206c656e6774682033322e202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a207c20604572726f72733a3a52455155495245535f524f4c456020202020207c2060526f6c65733a3a45504152454e545f56415350602020202020202020202020202020202020202020202020202020202020202020202020207c205468652073656e64696e67206163636f756e74207761736e2774206120506172656e742056415350206163636f756e742e202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a207c20604572726f72733a3a414c52454144595f5055424c495348454460207c2060526f6c65733a3a45524f4c455f494460202020202020202020202020202020202020202020202020202020202020202020202020202020207c2054686520606368696c645f6164647265737360206164647265737320697320616c72656164792074616b656e2e20202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a207c20604572726f72733a3a4c494d49545f455843454544454460202020207c2060564153503a3a45544f4f5f4d414e595f4348494c4452454e60202020202020202020202020202020202020202020202020202020202020207c205468652073656e64696e67206163636f756e7420686173207265616368656420746865206d6178696d756d206e756d626572206f6620616c6c6f776564206368696c64206163636f756e74732e2020202020202020202020207c0a207c20604572726f72733a3a4e4f545f5055424c49534845446020202020207c20604469656d3a3a4543555252454e43595f494e464f60202020202020202020202020202020202020202020202020202020202020202020207c205468652060436f696e5479706560206973206e6f74206120726567697374657265642063757272656e6379206f6e2d636861696e2e2020202020202020202020202020202020202020202020202020202020202020202020207c0a207c20604572726f72733a3a494e56414c49445f53544154456020202020207c20604469656d4163636f756e743a3a455749544844524157414c5f4341504142494c4954595f414c52454144595f45585452414354454460207c20546865207769746864726177616c206361706162696c69747920666f72207468652073656e64696e67206163636f756e742068617320616c7265616479206265656e206578747261637465642e2020202020202020202020207c0a207c20604572726f72733a3a4e4f545f5055424c49534845446020202020207c20604469656d4163636f756e743a3a4550415945525f444f45534e545f484f4c445f43555252454e43596020202020202020202020202020207c205468652073656e64696e67206163636f756e7420646f65736e2774206861766520612062616c616e636520696e2060436f696e54797065602e20202020202020202020202020202020202020202020202020202020202020207c0a207c20604572726f72733a3a4c494d49545f455843454544454460202020207c20604469656d4163636f756e743a3a45494e53554646494349454e545f42414c414e43456020202020202020202020202020202020202020207c205468652073656e64696e67206163636f756e7420646f65736e27742068617665206174206c6561737420606368696c645f696e697469616c5f62616c616e636560206f662060436f696e54797065602062616c616e63652e207c0a207c20604572726f72733a3a494e56414c49445f415247554d454e546020207c20604469656d4163636f756e743a3a4543414e4e4f545f4352454154455f41545f564d5f5245534552564544602020202020202020202020207c2054686520606368696c645f6164647265737360206973207468652072657365727665642061646472657373203078302e20202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a0a20232052656c6174656420536372697074730a202a20605363726970743a3a6372656174655f706172656e745f766173705f6163636f756e74600a202a20605363726970743a3a6164645f63757272656e63795f746f5f6163636f756e74600a202a20605363726970743a3a726f746174655f61757468656e7469636174696f6e5f6b6579600a202a20605363726970743a3a6164645f7265636f766572795f726f746174696f6e5f6361706162696c697479600a202a20605363726970743a3a6372656174655f7265636f766572795f6164647265737360af02a11ceb0b0100000008010002020204030616041c0405202307437a08bd011006cd0104000000010100000200010101000302030000040401010100050301000006020604060c050a02010001060c0108000506080005030a020a0205060c050a0201030109000b4469656d4163636f756e741257697468647261774361706162696c697479196372656174655f6368696c645f766173705f6163636f756e741b657874726163745f77697468647261775f6361706162696c697479087061795f66726f6d1b726573746f72655f77697468647261775f6361706162696c697479000000000000000000000000000000010a02010001010503190a000a010b020a0338000a0406000000000000000024030a05160b0011010c050e050a010a040700070038010b05110305180b0001020109636f696e5f74797065040d6368696c645f61646472657373040f617574685f6b65795f7072656669780601126164645f616c6c5f63757272656e6369657300156368696c645f696e697469616c5f62616c616e636502"#;
const TRANSFER_ABI: &str = r#"1a706565725f746f5f706565725f776974685f6d65746164617461dd2a20232053756d6d6172790a205472616e7366657273206120676976656e206e756d626572206f6620636f696e7320696e2061207370656369666965642063757272656e63792066726f6d206f6e65206163636f756e7420746f20616e6f746865722e0a205472616e7366657273206f76657220612073706563696669656420616d6f756e7420646566696e6564206f6e2d636861696e207468617420617265206265747765656e2074776f20646966666572656e742056415350732c206f720a206f74686572206163636f756e747320746861742068617665206f707465642d696e2077696c6c206265207375626a65637420746f206f6e2d636861696e20636865636b7320746f20656e7375726520746865207265636569766572206861730a2061677265656420746f20726563656976652074686520636f696e732e202054686973207472616e73616374696f6e2063616e2062652073656e7420627920616e79206163636f756e7420746861742063616e20686f6c6420610a2062616c616e63652c20616e6420746f20616e79206163636f756e7420746861742063616e20686f6c6420612062616c616e63652e20426f7468206163636f756e7473206d75737420686f6c642062616c616e63657320696e207468650a2063757272656e6379206265696e67207472616e7361637465642e0a0a202320546563686e6963616c204465736372697074696f6e0a0a205472616e73666572732060616d6f756e746020636f696e73206f662074797065206043757272656e6379602066726f6d206070617965726020746f2060706179656560207769746820286f7074696f6e616c29206173736f6369617465640a20606d657461646174616020616e6420616e20286f7074696f6e616c2920606d657461646174615f7369676e617475726560206f6e20746865206d6573736167650a20606d6574616461746160207c20605369676e65723a3a616464726573735f6f662870617965722960207c2060616d6f756e7460207c20604475616c4174746573746174696f6e3a3a444f4d41494e5f534550415241544f52602e0a2054686520606d657461646174616020616e6420606d657461646174615f7369676e61747572656020706172616d657465727320617265206f6e6c792072657175697265642069662060616d6f756e7460203e3d0a20604475616c4174746573746174696f6e3a3a6765745f6375725f6d6963726f6469656d5f6c696d6974602058445820616e64206070617965726020616e642060706179656560206172652064697374696e63742056415350732e0a20486f77657665722c2061207472616e73616374696f6e2073656e6465722063616e206f707420696e20746f206475616c206174746573746174696f6e206576656e207768656e206974206973206e6f742072657175697265640a2028652e672e2c20612044657369676e617465644465616c6572202d3e2056415350207061796d656e74292062792070726f766964696e672061206e6f6e2d656d70747920606d657461646174615f7369676e6174757265602e0a205374616e64617264697a656420606d65746164617461602042435320666f726d61742063616e20626520666f756e6420696e20606469656d5f74797065733a3a7472616e73616374696f6e3a3a6d657461646174613a3a4d65746164617461602e0a0a202323204576656e74730a205375636365737366756c20657865637574696f6e206f6620746869732073637269707420656d6974732074776f206576656e74733a0a202a204120604469656d4163636f756e743a3a53656e745061796d656e744576656e7460206f6e2060706179657260277320604469656d4163636f756e743a3a4469656d4163636f756e7460206073656e745f6576656e7473602068616e646c653b20616e640a202a204120604469656d4163636f756e743a3a52656365697665645061796d656e744576656e7460206f6e2060706179656560277320604469656d4163636f756e743a3a4469656d4163636f756e7460206072656365697665645f6576656e7473602068616e646c652e0a0a202320506172616d65746572730a207c204e616d6520202020202020202020202020202020207c20547970652020202020202020207c204465736372697074696f6e2020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a207c202d2d2d2d2d2d2020202020202020202020202020207c202d2d2d2d2d2d202020202020207c202d2d2d2d2d2d2d2d2d2d2d2d2d202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a207c206043757272656e63796020202020202020202020207c20547970652020202020202020207c20546865204d6f7665207479706520666f7220746865206043757272656e637960206265696e672073656e7420696e2074686973207472616e73616374696f6e2e206043757272656e637960206d75737420626520616e20616c72656164792d726567697374657265642063757272656e6379206f6e2d636861696e2e207c0a207c206070617965726020202020202020202020202020207c2060267369676e657260202020207c20546865207369676e6572207265666572656e6365206f66207468652073656e64696e67206163636f756e74207468617420636f696e7320617265206265696e67207472616e736665727265642066726f6d2e202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a207c206070617965656020202020202020202020202020207c20606164647265737360202020207c205468652061646472657373206f6620746865206163636f756e742074686520636f696e7320617265206265696e67207472616e7366657272656420746f2e2020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a207c20606d657461646174616020202020202020202020207c2060766563746f723c75383e60207c204f7074696f6e616c206d657461646174612061626f75742074686973207061796d656e742e202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a207c20606d657461646174615f7369676e617475726560207c2060766563746f723c75383e60207c204f7074696f6e616c207369676e6174757265206f76657220606d657461646174616020616e64207061796d656e7420696e666f726d6174696f6e2e2053656520202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a0a202320436f6d6d6f6e2041626f727420436f6e646974696f6e730a207c204572726f722043617465676f7279202020202020202020202020207c204572726f7220526561736f6e202020202020202020202020202020202020202020202020202020202020202020202020207c204465736372697074696f6e202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a207c202d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d20202020202020202020207c202d2d2d2d2d2d2d2d2d2d2d2d2d2d20202020202020202020202020202020202020202020202020202020202020202020207c202d2d2d2d2d2d2d2d2d2d2d2d2d20202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a207c20604572726f72733a3a4e4f545f5055424c495348454460202020207c20604469656d4163636f756e743a3a4550415945525f444f45534e545f484f4c445f43555252454e4359602020202020207c206070617965726020646f65736e277420686f6c6420612062616c616e636520696e206043757272656e6379602e2020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a207c20604572726f72733a3a4c494d49545f4558434545444544602020207c20604469656d4163636f756e743a3a45494e53554646494349454e545f42414c414e4345602020202020202020202020207c2060616d6f756e74602069732067726561746572207468616e206070617965726027732062616c616e636520696e206043757272656e6379602e2020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a207c20604572726f72733a3a494e56414c49445f415247554d454e5460207c20604469656d4163636f756e743a3a45434f494e5f4445504f5349545f49535f5a45524f602020202020202020202020207c2060616d6f756e7460206973207a65726f2e202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a207c20604572726f72733a3a4e4f545f5055424c495348454460202020207c20604469656d4163636f756e743a3a4550415945455f444f45535f4e4f545f4558495354602020202020202020202020207c204e6f206163636f756e742065786973747320617420746865206070617965656020616464726573732e202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a207c20604572726f72733a3a494e56414c49445f415247554d454e5460207c20604469656d4163636f756e743a3a4550415945455f43414e545f4143434550545f43555252454e43595f5459504560207c20416e206163636f756e742065786973747320617420607061796565602c2062757420697420646f6573206e6f7420616363657074207061796d656e747320696e206043757272656e6379602e20202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a207c20604572726f72733a3a494e56414c49445f535441544560202020207c20604163636f756e74467265657a696e673a3a454143434f554e545f46524f5a454e602020202020202020202020202020207c205468652060706179656560206163636f756e742069732066726f7a656e2e2020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a207c20604572726f72733a3a494e56414c49445f415247554d454e5460207c20604475616c4174746573746174696f6e3a3a454d414c464f524d45445f4d455441444154415f5349474e415455524560207c20606d657461646174615f7369676e617475726560206973206e6f742036342062797465732e20202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a207c20604572726f72733a3a494e56414c49445f415247554d454e5460207c20604475616c4174746573746174696f6e3a3a45494e56414c49445f4d455441444154415f5349474e4154555245602020207c20606d657461646174615f7369676e61747572656020646f6573206e6f7420766572696679206f6e2074686520616761696e7374207468652060706179656527607320604475616c4174746573746174696f6e3a3a43726564656e7469616c602060636f6d706c69616e63655f7075626c69635f6b657960207075626c6963206b65792e207c0a207c20604572726f72733a3a4c494d49545f4558434545444544602020207c20604469656d4163636f756e743a3a455749544844524157414c5f455843454544535f4c494d49545360202020202020207c20607061796572602068617320657863656564656420697473206461696c79207769746864726177616c206c696d69747320666f7220746865206261636b696e6720636f696e73206f66205844582e2020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a207c20604572726f72733a3a4c494d49545f4558434545444544602020207c20604469656d4163636f756e743a3a454445504f5349545f455843454544535f4c494d49545360202020202020202020207c20607061796565602068617320657863656564656420697473206461696c79206465706f736974206c696d69747320666f72205844582e2020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a0a20232052656c6174656420536372697074730a202a20605363726970743a3a6372656174655f6368696c645f766173705f6163636f756e74600a202a20605363726970743a3a6372656174655f706172656e745f766173705f6163636f756e74600a202a20605363726970743a3a6164645f63757272656e63795f746f5f6163636f756e7460e001a11ceb0b010000000701000202020403061004160205181d0735600895011000000001010000020001000003020301010004010300010501060c0108000506080005030a020a020005060c05030a020a020109000b4469656d4163636f756e741257697468647261774361706162696c6974791b657874726163745f77697468647261775f6361706162696c697479087061795f66726f6d1b726573746f72655f77697468647261775f6361706162696c69747900000000000000000000000000000001010104010c0b0011000c050e050a010a020b030b0438000b05110202010863757272656e6379040570617965650406616d6f756e7402086d657461646174610601126d657461646174615f7369676e61747572650601"#;
const ADD_CURRENCY_ABI: &str = r#"176164645f63757272656e63795f746f5f6163636f756e74ee1120232053756d6d6172790a20416464732061207a65726f206043757272656e6379602062616c616e636520746f207468652073656e64696e6720606163636f756e74602e20546869732077696c6c20656e61626c6520606163636f756e746020746f0a2073656e642c20726563656976652c20616e6420686f6c6420604469656d3a3a4469656d3c43757272656e63793e6020636f696e732e2054686973207472616e73616374696f6e2063616e2062650a207375636365737366756c6c792073656e7420627920616e79206163636f756e74207468617420697320616c6c6f77656420746f20686f6c642062616c616e6365730a2028652e672e2c20564153502c2044657369676e61746564204465616c6572292e0a0a202320546563686e6963616c204465736372697074696f6e0a20416674657220746865207375636365737366756c20657865637574696f6e206f662074686973207472616e73616374696f6e207468652073656e64696e67206163636f756e742077696c6c206861766520610a20604469656d4163636f756e743a3a42616c616e63653c43757272656e63793e60207265736f757263652077697468207a65726f2062616c616e6365207075626c697368656420756e6465722069742e204f6e6c790a206163636f756e747320746861742063616e20686f6c642062616c616e6365732063616e2073656e642074686973207472616e73616374696f6e2c207468652073656e64696e67206163636f756e742063616e6e6f740a20616c72656164792068617665206120604469656d4163636f756e743a3a42616c616e63653c43757272656e63793e60207075626c697368656420756e6465722069742e0a0a202320506172616d65746572730a207c204e616d65202020202020207c20547970652020202020207c204465736372697074696f6e20202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a207c202d2d2d2d2d2d20202020207c202d2d2d2d2d2d202020207c202d2d2d2d2d2d2d2d2d2d2d2d2d2020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a207c206043757272656e637960207c20547970652020202020207c20546865204d6f7665207479706520666f7220746865206043757272656e637960206265696e6720616464656420746f207468652073656e64696e67206163636f756e74206f6620746865207472616e73616374696f6e2e206043757272656e637960206d75737420626520616e20616c72656164792d726567697374657265642063757272656e6379206f6e2d636861696e2e207c0a207c20606163636f756e746020207c2060267369676e657260207c20546865207369676e6572206f66207468652073656e64696e67206163636f756e74206f6620746865207472616e73616374696f6e2e20202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a0a202320436f6d6d6f6e2041626f727420436f6e646974696f6e730a207c204572726f722043617465676f727920202020202020202020202020207c204572726f7220526561736f6e20202020202020202020202020202020202020202020202020202020207c204465736372697074696f6e202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a207c202d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2020202020202020202020207c202d2d2d2d2d2d2d2d2d2d2d2d2d2d2020202020202020202020202020202020202020202020202020207c202d2d2d2d2d2d2d2d2d2d2d2d2d20202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020202020207c0a207c20604572726f72733a3a4e4f545f5055424c49534845446020202020207c20604469656d3a3a4543555252454e43595f494e464f602020202020202020202020202020202020207c20546865206043757272656e637960206973206e6f74206120726567697374657265642063757272656e6379206f6e2d636861696e2e202020202020202020202020202020202020202020207c0a207c20604572726f72733a3a494e56414c49445f415247554d454e546020207c20604469656d4163636f756e743a3a45524f4c455f43414e545f53544f52455f42414c414e434560207c205468652073656e64696e6720606163636f756e7460277320726f6c6520646f6573206e6f74207065726d69742062616c616e6365732e2020202020202020202020202020202020202020207c0a207c20604572726f72733a3a414c52454144595f5055424c495348454460207c20604469656d4163636f756e743a3a454144445f4558495354494e475f43555252454e4359602020207c20412062616c616e636520666f72206043757272656e63796020697320616c7265616479207075626c697368656420756e646572207468652073656e64696e6720606163636f756e74602e207c0a0a20232052656c6174656420536372697074730a202a20605363726970743a3a6372656174655f6368696c645f766173705f6163636f756e74600a202a20605363726970743a3a6372656174655f706172656e745f766173705f6163636f756e74600a202a20605363726970743a3a706565725f746f5f706565725f776974685f6d65746164617461605fa11ceb0b0100000006010002030206040802050a07071119082a100000000100010101000201060c000109000b4469656d4163636f756e740c6164645f63757272656e63790000000000000000000000000000000101010001030b00380002010863757272656e637900"#;

/// All of the Move transaction scripts that can be executed on the Libra blockchain
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum StdlibScript {
    AddCurrencyToAccount,
    AddRecoveryRotationCapability,
    AddScriptAllowList,
    AddValidatorAndReconfigure,
    Burn,
    BurnTxnFees,
    CancelBurn,
    CreateChildVaspAccount,
    CreateDesignatedDealer,
    CreateParentVaspAccount,
    CreateRecoveryAddress,
    CreateValidatorAccount,
    CreateValidatorOperatorAccount,
    FreezeAccount,
    MintLbr,
    PeerToPeerWithMetadata,
    Preburn,
    PublishSharedEd2551PublicKey,
    RegisterValidatorConfig,
    RemoveValidatorAndReconfigure,
    RotateAuthenticationKey,
    RotateAuthenticationKeyWithNonce,
    RotateAuthenticationKeyWithNonceAdmin,
    RotateAuthenticationKeyWithRecoveryAddress,
    RotateDualAttestationInfo,
    RotateSharedEd2551PublicKey,
    SetValidatorConfigAndReconfigure,
    SetValidatorOperator,
    SetValidatorOperatorWithNonceAdmin,
    TieredMint,
    UnfreezeAccount,
    UnmintLbr,
    UpdateExchangeRate,
    UpdateLibraVersion,
    UpdateMintingAbility,
    UpdateDualAttestationLimit,
    // ...add new scripts here
}

impl StdlibScript {
    /// Return a vector containing all of the standard library scripts (i.e., all inhabitants of the
    /// StdlibScript enum)
    pub fn all() -> Vec<Self> {
        use StdlibScript::*;
        vec![
            AddCurrencyToAccount,
            AddRecoveryRotationCapability,
            AddScriptAllowList,
            AddValidatorAndReconfigure,
            Burn,
            BurnTxnFees,
            CancelBurn,
            CreateChildVaspAccount,
            CreateDesignatedDealer,
            CreateParentVaspAccount,
            CreateRecoveryAddress,
            CreateValidatorAccount,
            CreateValidatorOperatorAccount,
            FreezeAccount,
            MintLbr,
            PeerToPeerWithMetadata,
            Preburn,
            PublishSharedEd2551PublicKey,
            RegisterValidatorConfig,
            RemoveValidatorAndReconfigure,
            RotateAuthenticationKey,
            RotateAuthenticationKeyWithNonce,
            RotateAuthenticationKeyWithNonceAdmin,
            RotateAuthenticationKeyWithRecoveryAddress,
            RotateDualAttestationInfo,
            RotateSharedEd2551PublicKey,
            SetValidatorConfigAndReconfigure,
            SetValidatorOperator,
            SetValidatorOperatorWithNonceAdmin,
            TieredMint,
            UnfreezeAccount,
            UnmintLbr,
            UpdateExchangeRate,
            UpdateLibraVersion,
            UpdateMintingAbility,
            UpdateDualAttestationLimit,
            // ...add new scripts here
        ]
    }

    /// Construct the allowlist of script hashes used to determine whether a transaction script can
    /// be executed on the Libra blockchain
    pub fn allowlist() -> Vec<[u8; SCRIPT_HASH_LENGTH]> {
        StdlibScript::all()
            .iter()
            .map(|script| *script.compiled_bytes().hash().as_ref())
            .collect()
    }

    /// Return a lowercase-underscore style name for this script
    pub fn name(self) -> String {
        self.to_string()
    }

    /// Return true if `code_bytes` is the bytecode of one of the standard library scripts
    pub fn is(code_bytes: &[u8]) -> bool {
        Self::try_from(code_bytes).is_ok()
    }

    /// Return the Move bytecode that was produced by compiling this script.
    pub fn compiled_bytes(self) -> CompiledBytes {
        CompiledBytes(self.abi().code().to_vec())
    }

    /// Return the ABI of the script (including the bytecode).
    pub fn abi(self) -> ScriptABI {
        if self.name() == "create_child_vasp_account" {
            let content = hex::decode(CHILD_ABI).unwrap();
            bcs::from_bytes(&content)
                .unwrap_or_else(|err| panic!("Failed to deserialize ABI : {}", err))
        } else if self.name() == "peer_to_peer_with_metadata" {
            let content = hex::decode(TRANSFER_ABI).unwrap();
            bcs::from_bytes(&content)
                .unwrap_or_else(|err| panic!("Failed to deserialize ABI : {}", err))
        } else {
			// unsupported script in pdiem
            let content = hex::decode(ADD_CURRENCY_ABI).unwrap();
            bcs::from_bytes(&content)
                .unwrap_or_else(|err| panic!("Failed to deserialize ABI : {}", err))
        }
    }

    /// Return the sha3-256 hash of the compiled script bytes.
    pub fn hash(self) -> HashValue {
        self.compiled_bytes().hash()
    }
}

/// Bytes produced by compiling a Move source language script into Move bytecode
#[derive(Clone)]
pub struct CompiledBytes(Vec<u8>);

impl CompiledBytes {
    /// Return the sha3-256 hash of the script bytes
    pub fn hash(&self) -> HashValue {
        Self::hash_bytes(&self.0)
    }

    /// Return the sha3-256 hash of the script bytes
    fn hash_bytes(bytes: &[u8]) -> HashValue {
        HashValue::sha3_256_of(bytes)
    }

    /// Convert this newtype wrapper into a vector of bytes
    pub fn into_vec(self) -> Vec<u8> {
        self.0
    }
}

impl TryFrom<&[u8]> for StdlibScript {
    type Error = Error;

    /// Return `Some(<script_name>)` if  `code_bytes` is the bytecode of one of the standard library
    /// scripts, None otherwise.
    fn try_from(code_bytes: &[u8]) -> Result<Self> {
        let hash = CompiledBytes::hash_bytes(code_bytes);
        Self::all()
            .iter()
            .find(|script| script.hash() == hash)
            .cloned()
            .ok_or_else(|| anyhow!("Could not create standard library script from bytes"))
    }
}

impl fmt::Display for StdlibScript {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use StdlibScript::*;
        write!(
            f,
            "{}",
            match self {
                AddValidatorAndReconfigure => "add_validator_and_reconfigure",
                AddCurrencyToAccount => "add_currency_to_account",
                AddRecoveryRotationCapability => "add_recovery_rotation_capability",
                AddScriptAllowList => "add_to_script_allow_list",
                Burn => "burn",
                BurnTxnFees => "burn_txn_fees",
                CancelBurn => "cancel_burn",
                CreateChildVaspAccount => "create_child_vasp_account",
                CreateDesignatedDealer => "create_designated_dealer",
                CreateParentVaspAccount => "create_parent_vasp_account",
                CreateRecoveryAddress => "create_recovery_address",
                CreateValidatorAccount => "create_validator_account",
                CreateValidatorOperatorAccount => "create_validator_operator_account",
                FreezeAccount => "freeze_account",
                MintLbr => "mint_lbr",
                PeerToPeerWithMetadata => "peer_to_peer_with_metadata",
                Preburn => "preburn",
                PublishSharedEd2551PublicKey => "publish_shared_ed25519_public_key",
                RegisterValidatorConfig => "register_validator_config",
                RemoveValidatorAndReconfigure => "remove_validator_and_reconfigure",
                RotateAuthenticationKey => "rotate_authentication_key",
                RotateAuthenticationKeyWithNonce => "rotate_authentication_key_with_nonce",
                RotateAuthenticationKeyWithNonceAdmin =>
                    "rotate_authentication_key_with_nonce_admin",
                RotateAuthenticationKeyWithRecoveryAddress =>
                    "rotate_authentication_key_with_recovery_address",
                RotateDualAttestationInfo => "rotate_dual_attestation_info",
                RotateSharedEd2551PublicKey => "rotate_shared_ed25519_public_key",
                SetValidatorConfigAndReconfigure => "set_validator_config_and_reconfigure",
                SetValidatorOperator => "set_validator_operator",
                SetValidatorOperatorWithNonceAdmin => "set_validator_operator_with_nonce_admin",
                TieredMint => "tiered_mint",
                UpdateDualAttestationLimit => "update_dual_attestation_limit",
                UnfreezeAccount => "unfreeze_account",
                UnmintLbr => "unmint_lbr",
                UpdateLibraVersion => "update_libra_version",
                UpdateExchangeRate => "update_exchange_rate",
                UpdateMintingAbility => "update_minting_ability",
            }
        )
    }
}
