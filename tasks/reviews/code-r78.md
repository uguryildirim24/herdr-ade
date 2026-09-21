+++
verdict = "MERGE"
round = "r78"
candidate = "7131f535749b5c7d5b7a580c079cffd015b1ff12"
manifest_hash = "d03051d2083fc99fd99f376b29bec1d04dc54cac13d4bab978fbce8072822ba7"
policy_hash = "8e43e0ebbd105ebf9f903667c622cb0dad8aa881e4ceb5a344db4659ff0338da"
gates = []
+++

`t-0184` now has one evidence-derived task projection across the generated task list, context, plan state, and talk screen. Task records carry no writable status, review and merge come from round evidence, and install and verification require their own evidence.

The review fixes first-generation migration so no live hand-written task list is replaced before a task exists, and its exact bytes are archived and named when generation begins. It also prevents an unreadable task record from losing its stable identity during later allocation. Historical threads remain readable without task links. The requested box gates passed; their commands and results are in the reviewer report.
