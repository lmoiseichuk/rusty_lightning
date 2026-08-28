# file DFRobot_AS3935_Lib.py
#
# SEN0290 Lightning Sensor
#
#
# Copyright    [DFRobot](http://www.dfrobot.com), 2018
# Copyright    GNU Lesser General Public License
#
# version  V1.0
# date  2018-11-28
#
# Modified by Leonid Moiseichuk to minimize print() calls and float operations, add checks

import utime
from machine import Pin, I2C

class DFRobot_AS3935:
    def __init__(self, address, i2c):
        self.address = address
        self.i2cbus = i2c

    def writeByte(self, register, value) -> bool:
        try:
            self.i2cbus.writeto_mem(self.address, register, bytes([value]))
            return True
        except:
            return False

    def readByte(self, register) -> bool:
        try:
            self.register = self.i2cbus.readfrom_mem(self.address, register, 1)
            return True
        except:
            return False

    def manualCal(self, capacitance, location, disturber):
        self.powerUp()
        if location == 1:
            self.setIndoors()
        else:
            self.setOutdoors()
        if disturber == 0:
            self.disturberDis()
        else:
            self.disturberEn()
        self.setIrqOutputSource(0)
        utime.sleep_ms(500)
        self.setTuningCaps(capacitance)

    def setTuningCaps(self, capVal) -> bool:
        # Assume only numbers divisible by 8 (because that's all the chip supports)
        # set capacitance bits to maximum if more that 120
        value = 0x0F if capVal > 120 else (capVal >> 3)
        return self.singRegWrite(0x08, 0x0F, value) and self.singRegRead(0x08)

    def powerUp(self) -> bool:
        #register 0x00, PWD bit: 0 (clears PWD)
        if not self.singRegWrite(0x00, 0x01, 0x00):
            return False
        if not self.calRCO(): #run RCO cal cmd
            return False
        if not self.singRegWrite(0x08, 0x20, 0x20): #set DISP_SRCO to 1
            return False
        utime.sleep_ms(2)
        return self.singRegWrite(0x08, 0x20, 0x00) #set DISP_SRCO to 0


    def powerDown(self) -> bool:
        #register 0x00, PWD bit: 0 (sets PWD)
        return self.singRegWrite(0x00, 0x01, 0x01)

    def calRCO(self) -> bool:
        isOk = self.writeByte(0x3D, 0x96)
        utime.sleep_ms(2)
        return isOk

    def setIndoors(self) -> bool:
        return self.singRegWrite(0x00, 0x3E, 0x24)

    def setOutdoors(self) -> bool:
        return self.singRegWrite(0x00, 0x3E, 0x1C)

    def disturberDis(self) -> bool:
        #register 0x03, PWD bit: 5 (sets MASK_DIST)
        return self.singRegWrite(0x03, 0x20, 0x20)

    def disturberEn(self) -> bool:
        #register 0x03, PWD bit: 5 (sets MASK_DIST)
        return self.singRegWrite(0x03, 0x20, 0x00)

    def singRegWrite(self, regAdd, dataMask, regData) -> bool:
        #start by reading original register data (only modifying what we need to)
        if not self.singRegRead(regAdd):
            return False
        #calculate new register data... 'delete' old targeted data, replace with new data
        #note: 'dataMask' must be bits targeted for replacement
        #add'l note: this function does NOT shift values into the proper place... they need to be there already
        newRegData = (self.register[0] & ~dataMask)|(regData & dataMask)
        #finally, write the data to the register
        return self.writeByte(regAdd, newRegData) and self.singRegRead(regAdd)

    def singRegRead(self, regAdd) -> bool:
        return self.readByte(regAdd)

    def getInterruptSrc(self) -> int:
        #definition of interrupt data on table 18 of datasheet
        #for this function:
        #0 = unknown src, 1 = lightning detected, 2 = disturber, 3 = Noise level too high
        utime.sleep_ms(3) #wait 3ms before reading (min 2ms per pg 22 of datasheet)
        if self.singRegRead(0x03): #read register, get rid of non-interrupt data
            intSrc = self.register[0] & 0x0F
            if intSrc == 0x08:
                return 1 #lightning caused interrupt
            elif intSrc == 0x04:
                return 2 #disturber detected
            elif intSrc == 0x01:
                return 3 #Noise level too high
        return 0 #interrupt result not expected

    def reset(self) -> bool:
        isOk = self.writeByte(0x3C, 0x96)
        utime.sleep_ms(2) #wait 2ms to complete
        return isOk

    def setLcoFdiv(self,fdiv) -> bool:
        return self.singRegWrite(0x03, 0xC0, (fdiv & 0x03) << 6)

    def setIrqOutputSource(self, irqSelect) -> bool:
        #set interrupt source - what to display on IRQ pin
        #reg 0x08, bits 5 (TRCO), 6 (SRCO), 7 (LCO)
        #only one should be set at once, I think
        #0 = NONE, 1 = TRCO, 2 = SRCO, 3 = LCO
        bitSet = 0x00   #clear IRQ pin display bits
        if irqSelect == 1:
            bitSet = 0x20   # set only TRCO bit
        elif irqSelect == 2:
            bitSet = 0x40   # set only SRCO bit
        elif irqSelect == 3:
            bitSet = 0x80   # set only SRCO bit
        return self.singRegWrite(0x08, 0xE0, bitSet)

    def getLightningDistKm(self) -> int:
        # read register, get rid of non-distance data
        return self.register[0] & 0x3F if self.singRegRead(0x07) else None

    def getStrikeEnergyRaw(self) -> float:
        if not self.singRegRead(0x06):  #MMSB, shift 8  bits left, make room for MSB
            return None
        nrgyRaw = (self.register[0]&0x1F) << 8
        if not self.singRegRead(0x05): # read MSB
            return None
        nrgyRaw |= self.register[0]
        nrgyRaw <<= 8 #shift 8 bits left, make room for LSB
        if not self.singRegRead(0x04): #read LSB, add to others
            return None
        nrgyRaw |= self.register[0]
        # OK, finalize and return
        return nrgyRaw / 16777

    def setMinStrikes(self, minStrk) -> int:
        #This function sets min strikes to the closest available number, rounding to the floor,
        #where necessary, then returns the physical value that was set. Options are 1, 5, 9 or 16 strikes.
        if minStrk < 5:
            return 1 if self.singRegWrite(0x02, 0x30, 0x00) else None
        if minStrk < 9:
            return 5 if self.singRegWrite(0x02, 0x30, 0x10) else None
        if minStrk < 16:
            return 9 if self.singRegWrite(0x02, 0x30, 0x20) else None
        return 16 if self.singRegWrite(0x02, 0x30, 0x30) else None

    def clearStatistics(self) -> bool:
        #clear is accomplished by toggling CL_STAT bit 'high-low-high' (then set low to move on)
        # high-low-high
        return self.singRegWrite(0x02, 0x40, 0x40) and self.singRegWrite(0x02, 0x40, 0x00) and self.singRegWrite(0x02, 0x40, 0x40)

    def getNoiseFloorLv1(self) -> int:
        #NF settings addres 0x01, bits 6:4
        #default setting of 010 at startup (datasheet, table 9)
        #read register 0x01 => should return value from 0-7, see table 16 for info
        return (self.register[0] & 0x70) >> 4 if self.singRegRead(0x01) else None

    def setNoiseFloorLv1(self, nfSel) -> bool:
        #NF settings addres 0x01, bits 6:4
        #default setting of 010 at startup (datasheet, table 9)
        # nfSel within expected range or out of range, set to default (power-up value 010)
        floorCode = (nfSel & 0x07) << 4 if nfSel <= 7 else 0x20
        return self.singRegWrite(0x01, 0x70, floorCode)
        
    def getWatchdogThreshold(self) -> int:
        #This function is used to read WDTH. It is used to increase robustness to disturbers,
        #though will make detection less efficient (see page 19, Fig 20 of datasheet)
        #WDTH register: add 0x01, bits 3:0
        #default value of 0010
        #values should only be between 0x00 and 0x0F (0 and 7)
        return self.register[0] & 0x0F if self.singRegRead(0x01) else None

    def setWatchdogThreshold(self, wdth) -> bool:
        #This function is used to modify WDTH. It is used to increase robustness to disturbers,
        #though will make detection less efficient (see page 19, Fig 20 of datasheet)
        #WDTH register: add 0x01, bits 3:0
        #default value of 0010
        #values should only be between 0x00 and 0x0F (0 and 7)
        return self.singRegWrite(0x01, 0x0F, wdth & 0x0F)

    def getSpikeRejection(self) -> int:
        #This function is used to read SREJ (spike rejection). Similar to the Watchdog threshold,
        #it is used to make the system more robust to disturbers, though will make general detection
        #less efficient (see page 20-21, especially Fig 21 of datasheet)
        #SREJ register: add 0x02, bits 3:0
        #default value of 0010
        #values should only be between 0x00 and 0x0F (0 and 7)
        return self.register[0] & 0x0F if self.singRegRead(0x02) else None

    def setSpikeRejection(self, srej) -> bool:
        #This function is used to modify SREJ (spike rejection). Similar to the Watchdog threshold,
        #it is used to make the system more robust to disturbers, though will make general detection
        #less efficient (see page 20-21, especially Fig 21 of datasheet)
        #WDTH register: add 0x02, bits 3:0
        #default value of 0010
        #values should only be between 0x00 and 0x0F (0 and 7)
        return self.singRegWrite(0x02, 0x0F, srej & 0x0F)
        
    def printAllRegs(self) -> bool:
        isOk = True
        for register in (0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x3A, 0x3B):
            if self.singRegRead(register):
                print("Reg 0x%02x: %02x" % (register, self.register[0]))
            else:
                print("Reg 0x%02x: READ FAILED" % register)
                isOk = False
        return isOk


