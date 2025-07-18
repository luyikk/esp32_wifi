use aqueue::Actor;
use esp_idf_svc::hal::gpio::{Output, Pin, PinDriver};

pub struct Led<G1: Pin> {
    pub led1: PinDriver<'static, G1, Output>,
}

impl<G1: Pin> Led<G1> {
    pub fn new(led2: PinDriver<'static, G1, Output>) -> Self {
        Led { led1: led2 }
    }
    fn set_led2_high(&mut self) -> anyhow::Result<()> {
        self.led1.set_high()?;
        Ok(())
    }

    fn set_led2_low(&mut self) -> anyhow::Result<()> {
        self.led1.set_low()?;
        Ok(())
    }
}

pub trait ILed {
    async fn led2_on(&self) -> anyhow::Result<()>;
    async fn led2_off(&self) -> anyhow::Result<()>;
}

impl<G1: Pin> ILed for Actor<Led<G1>> {
    async fn led2_on(&self) -> anyhow::Result<()> {
        self.inner_call(|inner| async move { inner.get_mut().set_led2_high() })
            .await
    }

    async fn led2_off(&self) -> anyhow::Result<()> {
        self.inner_call(|inner| async move { inner.get_mut().set_led2_low() })
            .await
    }
}
