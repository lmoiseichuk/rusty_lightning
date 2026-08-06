# file DFRobot_AS3935_detailed.py
#
# SEN0290 Lightning Sensor
# This sensor can detect lightning and display the distance and intensity of the lightning within 40 km
# It can be set as indoor or outdoor mode.
# The module has three I2C, these addresses are:
# AS3935_ADD1  0x01   A0 = 1  A1 = 0
# AS3935_ADD2  0x02   A0 = 0  A1 = 1
# AS3935_ADD3  0x03   A0 = 1  A1 = 1
#
#
# Copyright    [DFRobot](http://www.dfrobot.com), 2018
# Copyright    GNU Lesser General Public License
#
# version  V1.0
# date  2018-11-28

from machine import Pin, I2C
import utime

from hal import DFRobot_AS3935_Lib
#import DFRobot_AS3935_Lib

#I2C address
AS3935_I2C_ADDR1 = 0X01
AS3935_I2C_ADDR2 = 0X02
AS3935_I2C_ADDR3 = 0X03

# I2C HW ID or -1 for SW I2C
I2C_ID = 0
I2C_SCL_PIN = 9 # 22
I2C_SDA_PIN = 8 # 23

#Antenna tuning capcitance (must be integer multiple of 8, 8 - 120 pf)
AS3935_CAPACITANCE = 120
IRQ_PIN = 10 # 12

class Core:
    sensor = None
    events = list()   # basically what happened and details from irq

    # sensor parameters for calibration
    indoor = True
    noiseLevel = 0

    @staticmethod
    def tweakNoiseLevel(direction: int) -> bool:
        newLevel = Core.noiseLevel + direction
        if 0 <= newLevel <= 7:
            Core.noiseLevel = newLevel
            print('tweak noise level to', Core.noiseLevel)
            try:
                return Core.sensor.setNoiseFloorLv1(Core.noiseLevel)
            except Exception as oops:
                print('FAILED due to', oops)
        return False


    @staticmethod
    def gatherStrikeInfo() -> bool:
        try:
            Core.events.append([Core.sensor.getLightningDistKm(), sensor.getStrikeEnergyRaw()])
            return True
        except:
            # could be OSError: [Errno 19] ENODEV and so one
            return False


# Initialize 
i2c =  I2C(I2C_ID, scl=Pin(I2C_SCL_PIN), sda=Pin(I2C_SDA_PIN), freq=400000)
for addr in (AS3935_I2C_ADDR1, AS3935_I2C_ADDR2, AS3935_I2C_ADDR3):
  sensor = DFRobot_AS3935_Lib.DFRobot_AS3935(addr, i2c)
  if sensor.reset():
      print("init sensor sucess for " + str(addr))
      Core.sensor = sensor
      break
  else:
      print("init sensor fail for " + str(addr))

while Core.sensor is None:
   pass
#Configure sensor
Core.sensor.powerUp()

#set indoors or outdoors models
if Core.indoor:
    Core.sensor.setIndoors()
else:
    Core.sensor.setOutdoors()

#disturber detection
Core.sensor.disturberEn()
#sensor.disturberDis()

Core.sensor.setIrqOutputSource(0)
utime.sleep(0.5)
#set capacitance
Core.sensor.setTuningCaps(AS3935_CAPACITANCE)

# Connect the IRQ and GND pin to the oscilloscope.
# uncomment the following sentences to fine tune the antenna for better performance.
# This will dispaly the antenna's resonance frequency/16 on IRQ pin (The resonance frequency will be divided by 16 on this pin)
# Tuning AS3935_CAPACITANCE to make the frequency within 500/16 kHz plus 3.5% to 500/16 kHz minus 3.5%
#
# sensor.setLcoFdiv(0)
# sensor.setIrqOutputSource(3)

# Enable interrupt handling
def callback_handle(channel):
    try:
        utime.sleep(0.005)
        source = Core.sensor.getInterruptSrc()
        if source == 1:
            # extnded information, add without src
            Core.gatherStrikeInfo()
        else:
            # something not usual
            Core.events.append(source)
    except Exception as oops:
        print('data loading skipped due to exception', oops)

#Set to input mode
pinirq = Pin(IRQ_PIN, Pin.IN)
#Set the interrupt pin, the interrupt function, rising along the trigger
pinirq.irq(trigger=Pin.IRQ_RISING, handler=callback_handle)
print('the pin', pinirq, 'value is', pinirq.value())

#Set the noise level,use a default value greater than 7
noiseLv = Core.sensor.getNoiseFloorLv1()
print('get sensor noiseLv as', noiseLv)
Core.sensor.setNoiseFloorLv1(Core.noiseLevel)

#used to modify WDTH,alues should only be between 0x00 and 0x0F (0 and 7)
Core.sensor.setWatchdogThreshold(2)
#wtdgThreshold = sensor.getWatchdogThreshold()

#used to modify SREJ (spike rejection),values should only be between 0x00 and 0x0F (0 and 7)
Core.sensor.setSpikeRejection(0)
#spikeRejection = sensor.getSpikeRejection()

#view all register data
sensor.printAllRegs()

print("start lightning detect.")
activity = utime.time()
while True:
    utime.sleep(1.0)
    # we wakeup every second, so number of events is interesting
    if Core.events:
        # now swap event array contents
        activity = utime.time()
        events, Core.events = Core.events, list()
        stats = {'lightning': 0, 'distruber': 0, 'noise': 0}
        for event in events:
            if isinstance(event, list):
                # any element could be None -> workaround
                print('Lightning occurs: distance', event[0], 'km, intensity', event[1], ', score', (event[1] or 0)/(event[0] or 0.1))
                stats['lightning'] += 1
            elif event == 2:
                stats['distruber'] += 1
            elif event == 3:
                stats['noise'] += 1
            else:
                print('something strange', event)
        print('proceed', sum(stats.values()), 'events, details:', str(stats))
        # smart noise adjustment up
        if stats['distruber'] + stats['noise'] > 0:
            Core.tweakNoiseLevel(1)
    else:
        if activity + 60 < utime.time():
            # no events for last minute, tweak noise level down
            Core.tweakNoiseLevel(-1)
            activity = utime.time()
        print(*utime.localtime()[:6], ' => current noise level', Core.noiseLevel, ' ' * 10,  end='\r')
