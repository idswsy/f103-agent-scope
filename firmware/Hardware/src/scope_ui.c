/* 见 scope_ui.h 的文件头说明（许可、来源、注释重建）。 */

#if SCOPE_LOCAL_UI

#include "scope_ui.h"
#include "tft.h"
#include "tft_init.h"
#include "tim.h"
#include "adc.h"
#include "gpio.h"

/* ============ 原 Core/Inc/main.h 与 Core/Src/main.c ============ */

#define ADC_VALUE_NUM 300U

volatile struct Oscilloscope oscilloscope={0};
uint16_t adc_value[ADC_VALUE_NUM];

void Init_Oscilloscope(volatile struct Oscilloscope *value)
{
    (*value).showbit    = 0;                         //�����ʾ��־λ
    (*value).sampletime = ADC_SAMPLETIME_239CYCLES_5;//adc��������
    (*value).keyValue   = 0;                         //�������ֵ
    (*value).ouptputbit = 0;                         //�����־λ
    (*value).gatherFreq = 0;                         //�ɼ�Ƶ��
    (*value).outputFreq = 1000;                      //���Ƶ��
    (*value).pwmOut     = 500;                       //PWM���������PWMռ�ձ�
    (*value).timerPeriod= 1000;                      //PWM�����ʱ������
    (*value).vpp        = 0.0f;                      //���ֵ
}

/* ============ 原 Core/Src/gpio.c ============ */

static uint8_t key1_state = 0;
static uint8_t key2_state = 0;
static uint8_t key3_state = 0;
static uint8_t A_cnt = 0;
static uint8_t B_Value = 0;


void Open_Led(uint8_t value)
{
	switch(value)
	{
			case LED1:
					HAL_GPIO_WritePin(GPIOC,GPIO_PIN_14,GPIO_PIN_RESET);
					break;
			case LED2:
					HAL_GPIO_WritePin(GPIOC,GPIO_PIN_15,GPIO_PIN_RESET);
					break;
			default:
					break;
	}
}

void CLose_LED(uint8_t value)
{
    switch(value)
    {
        case LED1:
						HAL_GPIO_WritePin(GPIOC,GPIO_PIN_14,GPIO_PIN_SET);
            break;
        case LED2:
						HAL_GPIO_WritePin(GPIOC,GPIO_PIN_15,GPIO_PIN_SET);
            break;
        default:
            break;
    }
}

void Toggle_Led(uint8_t value)
{
		switch(value)
		{
				case LED1:
						HAL_GPIO_TogglePin(GPIOC,GPIO_PIN_14);
						break;
				case LED2:
						HAL_GPIO_TogglePin(GPIOC,GPIO_PIN_15);
						break;
				default:
						break;
		}
}

void Key_Handle(volatile struct Oscilloscope *value)
{
	ADC_ChannelConfTypeDef sConfig = {0};
	float tempValue=0;
	switch((*value).keyValue)
	{
		case KEY1PRESS:
		{
			(*value).pwmOut=((uint32_t)((*value).timerPeriod*0.04f))+(*value).pwmOut;
			if((*value).pwmOut > (*value).timerPeriod)
			{
					(*value).pwmOut = 0;
			}
			__HAL_TIM_SetCompare(&htim2, TIM_CHANNEL_3, (uint16_t)(*value).pwmOut);
		}
			break;
		case KEY2PRESS:
		{
			if((*value).ouptputbit == 0)
			{
				(*value).ouptputbit = 1;
				HAL_TIM_PWM_Start(&htim2,TIM_CHANNEL_3);
				//
			}
			else
			{
				(*value).ouptputbit = 0;
				HAL_TIM_PWM_Stop(&htim2,TIM_CHANNEL_3);
				//HAL_TIM_IC_Stop_IT(&htim3,TIM_CHANNEL_1);
			}
		}
			break;
		case KEY3PRESS:
		{			
			tempValue=(*value).pwmOut/((*value).timerPeriod+0.0f);
			(*value).timerPeriod = (*value).timerPeriod/2;
			if((*value).timerPeriod < 250)
			{
					(*value).timerPeriod = 1000;
			}
			(*value).outputFreq=1000000/(*value).timerPeriod;
			(*value).pwmOut=(uint16_t)((*value).timerPeriod*tempValue);
			__HAL_TIM_SetCompare(&htim2, TIM_CHANNEL_3, (uint16_t)(*value).pwmOut);
			__HAL_TIM_SetAutoreload(&htim2, (uint16_t)((*value).timerPeriod-1));
		}
			break;
		case KEYAPRESS:
		{
			switch((*value).sampletime)
			{
				case ADC_SAMPLETIME_239CYCLES_5:
					(*value).sampletime = ADC_SAMPLETIME_71CYCLES_5;
					break;
				case ADC_SAMPLETIME_71CYCLES_5:
					(*value).sampletime = ADC_SAMPLETIME_55CYCLES_5;
					break;
				case ADC_SAMPLETIME_55CYCLES_5:
					(*value).sampletime = ADC_SAMPLETIME_41CYCLES_5;
					break;
				case ADC_SAMPLETIME_41CYCLES_5:
					(*value).sampletime = ADC_SAMPLETIME_28CYCLES_5;
					break;
				case ADC_SAMPLETIME_28CYCLES_5:
					(*value).sampletime = ADC_SAMPLETIME_239CYCLES_5;
					break;
				default:
					(*value).sampletime=ADC_SAMPLETIME_239CYCLES_5;
					break;
			}
			sConfig.Channel = ADC_CHANNEL_3;
			sConfig.Rank = ADC_REGULAR_RANK_1;
			sConfig.SamplingTime = (*value).sampletime;
			if (HAL_ADC_ConfigChannel(&hadc1, &sConfig) != HAL_OK)
			{
				Error_Handler();
			}
		}
			break;
		case KEYBPRESS:
		{
			switch((*value).sampletime)
			{
				case ADC_SAMPLETIME_239CYCLES_5:
					(*value).sampletime = ADC_SAMPLETIME_28CYCLES_5;
					break;
				case ADC_SAMPLETIME_71CYCLES_5:
					(*value).sampletime = ADC_SAMPLETIME_239CYCLES_5;
					break;
				case ADC_SAMPLETIME_55CYCLES_5:
					(*value).sampletime = ADC_SAMPLETIME_71CYCLES_5;
					break;
				case ADC_SAMPLETIME_41CYCLES_5:
					(*value).sampletime = ADC_SAMPLETIME_55CYCLES_5;
					break;
				case ADC_SAMPLETIME_28CYCLES_5:
					(*value).sampletime = ADC_SAMPLETIME_41CYCLES_5;
					break;
				default:
					(*value).sampletime=ADC_SAMPLETIME_239CYCLES_5;
					break;
			}
			sConfig.Channel = ADC_CHANNEL_3;
			sConfig.Rank = ADC_REGULAR_RANK_1;
			sConfig.SamplingTime = (*value).sampletime;
			if (HAL_ADC_ConfigChannel(&hadc1, &sConfig) != HAL_OK)
			{
				Error_Handler();
			}
		}
			break;
		default:
			break;
	}
	(*value).keyValue=NoPRESS;
	TFT_ShowUI(value); 
}
void Key_Sacnf(volatile struct Oscilloscope *value)
{
	if(key1_state == KEYPRESS){
		HAL_Delay(20);
		if(HAL_GPIO_ReadPin(GPIOB,GPIO_PIN_13) == GPIO_PIN_RESET){
			(*value).keyValue = KEY1PRESS;
			key1_state = NoPRESS;
		}
	}
	else{
		key1_state = NoPRESS;
	}
	
	if(key2_state == KEYPRESS){
		HAL_Delay(20);
		if(HAL_GPIO_ReadPin(GPIOB,GPIO_PIN_14) == GPIO_PIN_RESET){
			Toggle_Led(LED1);
			(*value).keyValue = KEY2PRESS;
			key2_state = NoPRESS;
		}
	}
	else{
		key2_state = NoPRESS;
	}
	
	if(key3_state == KEYPRESS){
		HAL_Delay(20);
		if(HAL_GPIO_ReadPin(GPIOB,GPIO_PIN_15) == GPIO_PIN_RESET){
			(*value).keyValue = KEY3PRESS;
			key3_state = NoPRESS;
		}
	}
	else{
		key3_state = NoPRESS;
	}
}

void KEYD_SCAN(volatile struct Oscilloscope *value)
{
	if(HAL_GPIO_ReadPin(GPIOB,GPIO_PIN_9) == GPIO_PIN_RESET){
		HAL_Delay(20);
		if(HAL_GPIO_ReadPin(GPIOB,GPIO_PIN_9) == GPIO_PIN_RESET){
			while(HAL_GPIO_ReadPin(GPIOB,GPIO_PIN_9) == GPIO_PIN_RESET);
			(*value).keyValue = KEYDPRESS;
		}
	}
}

void HAL_GPIO_EXTI_Callback(uint16_t GPIO_Pin)
{
	if(GPIO_Pin == GPIO_PIN_4)
	{
		if((HAL_GPIO_ReadPin(GPIOB,GPIO_PIN_4) == GPIO_PIN_RESET) && (A_cnt == 0))	//A���½��ش���һ��
		{
			A_cnt++;
			B_Value = 0;
			if(HAL_GPIO_ReadPin(GPIOB,GPIO_PIN_3) == GPIO_PIN_SET){
				B_Value = 1;
			}
		}
		else if((HAL_GPIO_ReadPin(GPIOB,GPIO_PIN_4) == GPIO_PIN_SET) && (A_cnt == 1))	//A�������ش���һ��
		{
			A_cnt = 0;
			A_cnt = 0;
			if((B_Value == 1) && (HAL_GPIO_ReadPin(GPIOB,GPIO_PIN_3) == GPIO_PIN_RESET)){
				oscilloscope.keyValue=KEYBPRESS; 
			}
			if((B_Value == 0) && (HAL_GPIO_ReadPin(GPIOB,GPIO_PIN_3) == GPIO_PIN_SET)){
				oscilloscope.keyValue=KEYAPRESS;
			}
		}
	}
	if(GPIO_Pin == GPIO_PIN_13)
	{
		if(HAL_GPIO_ReadPin(GPIOB,GPIO_PIN_13) == GPIO_PIN_RESET){
			key1_state = KEYPRESS;
		}
	}
	if(GPIO_Pin == GPIO_PIN_14)
	{
		if(HAL_GPIO_ReadPin(GPIOB,GPIO_PIN_14) == GPIO_PIN_RESET){
			key2_state = KEYPRESS;
		}
	}
	if(GPIO_Pin == GPIO_PIN_15)
	{
		if(HAL_GPIO_ReadPin(GPIOB,GPIO_PIN_15) == GPIO_PIN_RESET){
			key3_state = KEYPRESS;
		}
	}
}

/* ============ 原 Core/Src/tim.c ============ */

static __IO uint16_t ccnumber = 0;
static __IO uint32_t freq = 0;
static __IO uint16_t readvalue1 = 0, readvalue2 = 0;
static __IO uint32_t count = 0;

void Freq_calibration(__IO uint32_t *freq)
{
	if(((*freq) >= 950) && ((*freq) < 1050)){
		(*freq) = 1000;
	}
	else if(((*freq) >= 1050) && ((*freq) < 2050)){
		(*freq) = 2000;
	}
	else if(((*freq) >= 2050) && ((*freq) < 3050)){
		(*freq) = 3000;
	}
	else if(((*freq) >= 3050) && ((*freq) < 4050)){
		(*freq) = 4000;
	}
	else if(((*freq) >= 4050) && ((*freq) < 5050)){
		(*freq) = 5000;
	}
	else if(((*freq) >= 5050) && ((*freq) < 6050)){
		(*freq) = 6000;
	}
	else if(((*freq) >= 6050) && ((*freq) < 7050)){
		(*freq) = 7000;
	}
	else if(((*freq) >= 7050) && ((*freq) < 8050)){
		(*freq) = 8000;
	}
	else if(((*freq) >= 8050) && ((*freq) < 9050)){
		(*freq) = 9000;
	}
	else if(((*freq) >= 9050) && ((*freq) < 10050)){
		(*freq) = 10000;
	}
}
void HAL_TIM_IC_CaptureCallback(TIM_HandleTypeDef *htim)
{
	if(htim->Instance == TIM3)
	{
		if(0 == ccnumber){
			readvalue1 = HAL_TIM_ReadCapturedValue(&htim3, TIM_CHANNEL_1);
			ccnumber = 1;
		}
		else if(1 == ccnumber)
		{
			readvalue2 = HAL_TIM_ReadCapturedValue(&htim3, TIM_CHANNEL_1);
			if(readvalue2 > readvalue1){
					count = (readvalue2 - readvalue1); 
			}else{
					count = ((0xFFFFU - readvalue1) + readvalue2); 
			}
			freq = 1000000U / count;
			Freq_calibration(&freq);
			oscilloscope.gatherFreq = freq; 			
			readvalue1 = 0;readvalue2 = 0;
			freq = 0;
			ccnumber = 0;
      count=0;
		}
	}
}

/* ============ 原 Core/Src/adc.c ============ */

void HAL_ADC_ConvCpltCallback(ADC_HandleTypeDef* hadc)
{
	if(hadc->Instance == ADC1)
	{
		oscilloscope.showbit=1;
		HAL_ADC_Stop_DMA(&hadc1);
	}
}

/* ============ 原 main() 的两段 ============ */

void ScopeUi_Setup(void)
{
	HAL_TIM_IC_Start_IT(&htim3,TIM_CHANNEL_1);
	Init_Oscilloscope(&oscilloscope);
	HAL_Delay(1000);
	TFT_Init();
	HAL_Delay(1000);	//��ʼ�����Σ�����ϵ�������⣬��������tft��λ��ͬ���������ڳ��γ�ʼ��ǰ�Ӵ���ʱ��Ч��
	TFT_Init();
	TFT_Fill(0,0,160,128,BLACK);
	TFT_StaticUI();
	
	HAL_ADC_Start_DMA(&hadc1, (uint32_t *)adc_value, ADC_VALUE_NUM);
}

void ScopeUi_Loop(void)
{
	/* 与上游一致：这些量在帧之间**保持**。`gainFactor` 只在 vpp>0.3 时更新，
	 * 小信号时沿用上一帧的值 —— 那是上游的既有行为，照搬不修。 */
	static uint16_t i = 0, Trigger_number = 0;
	static float tempValue = 0, max_data = 1.0f;
	static float gainFactor = 0, median = 0;
	static float voltage = 0, min = 0;
	static float calibration_vol = 0.15f;
		Key_Sacnf(&oscilloscope);
		Key_Handle(&oscilloscope);
		
		if(oscilloscope.showbit==1)
    {
			oscilloscope.showbit=0;
			oscilloscope.vpp=0;
			min = 9999;
			for(i=0;i<300;i++)
      {
				tempValue = (adc_value[i]*3.3f)/4095.0f;
				oscilloscope.voltageValue[i] = (5-(2.0f*tempValue));
				if((oscilloscope.vpp) < oscilloscope.voltageValue[i])
				{
						oscilloscope.vpp = oscilloscope.voltageValue[i];
				}
				if(min > oscilloscope.voltageValue[i])
				{
					min = oscilloscope.voltageValue[i];
				}
				if(oscilloscope.vpp <= 0.3)
				{
						oscilloscope.gatherFreq=0;
				}
			}
			oscilloscope.vpp = oscilloscope.vpp - calibration_vol;
			HAL_ADC_Start_DMA(&hadc1, (uint32_t *)adc_value, ADC_VALUE_NUM);
			
			for(i=0;i<200;i++)
			{
					if(oscilloscope.voltageValue[i] < max_data)
					{
							for(;i<200;i++)
							{
									if(oscilloscope.voltageValue[i] > max_data)
									{
											Trigger_number=i;
											break;
									}
							}
							break;
					}
			}
			
			if(oscilloscope.vpp > 0.3)
			{
				if(min < -0.3){
					median = oscilloscope.vpp;
				}else{
					median = oscilloscope.vpp / 2.0f;
				}
				//�Ŵ�������Ҫȷ���Ŵ�֮������䣬�ҽ����ι̶���ʾ�ڣ�18.75~41.25�У���(41.25-18.75)/2=11.25f
				gainFactor = 11.25f/median;
				
			}
			
			for(i=Trigger_number;i<Trigger_number+100;i++)
			{
					KEYD_SCAN(&oscilloscope);
					if(oscilloscope.keyValue == KEYDPRESS)
					{
							oscilloscope.keyValue = NoPRESS;
							do
							{
									KEYD_SCAN(&oscilloscope);
									if(oscilloscope.keyValue == KEYDPRESS){
											oscilloscope.keyValue = NoPRESS;
											break;
									}
							}while(1);
					}
					if(min < -0.3){
						voltage = oscilloscope.voltageValue[i] + oscilloscope.vpp;
					}
					else{
						voltage = oscilloscope.voltageValue[i];
					}									
					if(voltage >= median)
					{
							voltage = 30 + (voltage - median)*gainFactor;
					}
					else
					{
							voltage = 30 - (median - voltage)*gainFactor;
					}
					drawCurve(80,voltage);
			} 
		}
		
		TFT_ShowUI(&oscilloscope); 
  }

#endif /* SCOPE_LOCAL_UI */
