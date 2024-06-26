# event
# Built with Seahorse v0.1.0
#
# Emit on-chain events

from seahorse.prelude import *

declare_id('Fg6PaFpoGXkYsidMpWTK6W2BeZ7FEfcYkg476zPFsLnS')


@dataclass
class HelloEvent(Event):
    data: u8
    title: str
    owner: Pubkey


@instruction
def send_event(
    sender: Signer,
    data: u8,
    title: str
):
    event = HelloEvent(data, title, sender.key())
    event.emit()
